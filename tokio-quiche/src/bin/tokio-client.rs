use std::time::Duration;

use datagram_socket::ShutdownConnectionExt;
use foundations::telemetry::log;
use tokio_quiche::args::*;
use tokio_quiche::http3::driver::{
    ClientH3Event, H3Event, InboundFrame, IncomingH3Headers,
};
use tokio_quiche::http3::settings::Http3Settings;
use tokio_quiche::quic::ConnectionShutdownBehaviour;
use tokio_quiche::quiche::h3;
use tokio_quiche::settings::QuicSettings;
use tokio_quiche::ClientH3Driver;
use tokio_quiche::ConnectionParams;

#[tokio::main]
async fn main() -> tokio_quiche::QuicResult<()> {
    let docopt = docopt::Docopt::new(CLIENT_USAGE).unwrap();
    let conn_args = CommonArgs::with_docopt(&docopt);
    let args = ClientArgs::with_docopt(&docopt);
    // Create the configuration for the QUIC connection.

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

    let settings = QuicSettings::default();
    let mut params =
        ConnectionParams::new_client(settings, None, Default::default());


    params.settings.initial_rtt = Some(conn_args.initial_rtt);
    params.settings.logging_name = conn_args.logging_name;
    params.settings.max_idle_timeout =
        Some(Duration::from_millis(conn_args.idle_timeout));
    params.settings.cc_algorithm = conn_args.cc_algorithm;
    params.settings.initial_congestion_window_packets =
        conn_args.initial_cwnd_packets.try_into().unwrap();
    let (h3_driver, mut controller) =
        ClientH3Driver::new(Http3Settings::default());
    let socket = socket.try_into()?;

    println!("Path is: {:?}", file);

    let mut quic_connection =
        tokio_quiche::quic::connect_with_config(socket, None, &params, h3_driver)
            .await?;

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
                    log::info!("incoming headers"; "stream_id" => stream_id, "headers" => ?headers);
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
    quic_connection.shutdown_connection().await?;
    let send_application_close = true;
    let error_code = 0;
    let reason = Vec::new();
    let behaviour = ConnectionShutdownBehaviour {
        send_application_close,
        error_code,
        reason,
    };

    let _ = controller
        .cmd_sender()
        .send(tokio_quiche::quic::QuicCommand::ConnectionClose(behaviour));
    println!("connection close client!");

    Ok(())
}
