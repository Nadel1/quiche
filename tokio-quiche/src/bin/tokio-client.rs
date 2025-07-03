use std::time::Duration;

use foundations::telemetry::log;
use tokio_quiche::args::*;
use tokio_quiche::http3::driver::{
    ClientH3Event, H3Event, InboundFrame, IncomingH3Headers,
};
use tokio_quiche::http3::settings::Http3Settings;
use tokio_quiche::quiche::h3;
use tokio_quiche::socket::Socket;
use tokio_quiche::ClientH3Driver;
use tokio_quiche::settings::QuicSettings;
use tokio_quiche::ConnectionParams;

const MAX_DATAGRAM_SIZE: usize = 1350;

#[tokio::main]
async fn main() -> tokio_quiche::QuicResult<()> {
    println!("---beginning of client---");
    let docopt = docopt::Docopt::new(CLIENT_USAGE).unwrap();
    println!("new docopt");
    let conn_args = CommonArgs::with_docopt(&docopt);
    println!("new conn args");
    let args = ClientArgs::with_docopt(&docopt);
    println!("new args");
    // Create the configuration for the QUIC connection.

    //println!("new config");
    //config.set_application_protos(&conn_args.alpns).unwrap();
    //println!("new application protos");
    //println!("Set initial rtt: {:?}", conn_args.initial_rtt);
    //println!("Set max idle timeout: {:?}", conn_args.idle_timeout);
    //config.set_initial_rtt(conn_args.initial_rtt);
    //config.set_max_idle_timeout(conn_args.idle_timeout);
    //config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    //config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    //config.set_initial_max_data(conn_args.max_data);
    //config.set_initial_max_stream_data_bidi_local(conn_args.max_stream_data);
    //config.set_initial_max_stream_data_bidi_remote(conn_args.max_stream_data);
    //config.set_initial_max_stream_data_uni(conn_args.max_stream_data);
    //config.set_initial_max_streams_bidi(conn_args.max_streams_bidi);
    //config.set_initial_max_streams_uni(conn_args.max_streams_uni);
    //config.set_disable_active_migration(!conn_args.enable_active_migration);
    //config.set_active_connection_id_limit(conn_args.max_active_cids);
//
    //config.set_max_connection_window(conn_args.max_window);
    //config.set_max_stream_window(conn_args.max_stream_window);

    // We'll only connect to the first server provided in URL list.
    let connect_url = &args.urls[0];

    // Resolve server address.
    let peer_addr = if let Some(addr) = &args.connect_to {
        addr.parse().expect("--connect-to is expected to be a string containing an IPv4 or IPv6 address with a port. E.g. 192.0.2.0:443")
    } else {
        *connect_url.socket_addrs(|| None).unwrap().first().unwrap()
    };

    let bind_addr = match peer_addr {
        std::net::SocketAddr::V4(_) => format!("0.0.0.0:{}", args.source_port),
        std::net::SocketAddr::V6(_) => format!("[::]:{}", args.source_port),
    };
    let bind_to: String = bind_addr.parse().unwrap();
    let socket = tokio::net::UdpSocket::bind(bind_to).await?;
    let file = &args.urls[0].path();
    let mut file_path = file.to_string();
    if file_path.len() > 0 {
        file_path.remove(0); // remove first
    }
    println!("Connect url: {:}", peer_addr);
    println!("Args method: {}", &args.method);
    println!("Args method: {:?}", &args.dump_response_path);
    socket.connect(peer_addr).await?;
    //let mut params = ConnectionParams::default();
    let mut settings=QuicSettings::default();
    settings.max_idle_timeout=Some(Duration::from_millis(10000));
    settings.initial_rtt=Some(Duration::from_millis(10000));
    let mut params = ConnectionParams::new_client(
        settings,
        None,
        Default::default(),
    );
    params.settings.initial_rtt = Some(conn_args.initial_rtt);
    params.settings.max_idle_timeout =
        Some(Duration::from_millis(conn_args.idle_timeout));
    let (h3_driver, mut controller) =
        ClientH3Driver::new(Http3Settings::default());
    let socket = socket.try_into()?;

    //let (_, mut controller) = tokio_quiche::quic::connect(socket, None).await?;
    println!("Path is: {:?}", file);
    tokio_quiche::quic::connect_with_config(socket, None,&params, h3_driver)
        .await?;
    //println!("Set params, initial rtt: {:?} and idle timeout: {:?}", params.settings.initial_rtt,params.settings.max_idle_timeout);
    //let (_, mut controller) = tokio_quiche::quic::connect(socket, None).await?;
    println!("Connected");
    println!("Requests: {:?}", args.reqs_cardinal);

    for _i in 0..args.reqs_cardinal {
        controller
            .request_sender()
            .send(tokio_quiche::http3::driver::NewClientRequest {
                request_id: 0,
                headers: vec![
                    h3::Header::new(b":method", b"GET"),
                    h3::Header::new(b":path", file_path.as_bytes()),
                ],
                body_writer: None,
            })
            .unwrap();
        while let Some(event) = controller.event_receiver_mut().recv().await {
            match event {
                ClientH3Event::Core(H3Event::IncomingHeaders(
                    IncomingH3Headers {
                        stream_id,
                        headers,
                        mut recv,
                        ..
                    },
                )) => {
                    log::info!("incomming headers"; "stream_id" => stream_id, "headers" => ?headers);
                    'body: while let Some(frame) = recv.recv().await {
                        match frame {
                            InboundFrame::Body(pooled, fin) => {
                                log::info!("inbound body: {:?}", std::str::from_utf8(&pooled);
                                    "fin" => fin,
                                    "len" => pooled.len()
                                );
                                println!(
                                    "{}",
                                    std::str::from_utf8(&pooled).unwrap()
                                );
                                if fin {
                                    println!("received full body, exiting");
                                    break 'body;
                                }
                            },
                            InboundFrame::Datagram(pooled) => {
                                log::info!("inbound datagram"; "len" => pooled.len());
                            },
                        }
                    }
                },
                ClientH3Event::Core(H3Event::BodyBytesReceived {
                    fin: true,
                    ..
                }) => {
                    println!("fin received");
                    break;
                },
                ClientH3Event::Core(event) => {
                    log::info!("received event: {event:?}")
                },
                ClientH3Event::NewOutboundRequest {
                    stream_id,
                    request_id,
                } => log::info!(
                    "sending outbound request";
                    "stream_id" => stream_id,
                    "request_id" => request_id
                ),
            }
        }
    }
    Ok(())
}
