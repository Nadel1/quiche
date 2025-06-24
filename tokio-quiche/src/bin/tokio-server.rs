use foundations::telemetry::log;
use futures::{SinkExt as _, StreamExt as _};
use quiche::h3::NameValue;
use tokio_quiche::buf_factory::BufFactory;
use tokio_quiche::http3::driver::{
    H3Event, IncomingH3Headers, OutboundFrame, ServerH3Event,
};
use tokio_quiche::http3::settings::Http3Settings;
use tokio_quiche::listen;
use tokio_quiche::metrics::DefaultMetrics;
use tokio_quiche::quic::SimpleConnectionIdGenerator;
use tokio_quiche::quiche::h3;
use tokio_quiche::{ConnectionParams, ServerH3Controller, ServerH3Driver};

use tokio_quiche::args::*;
use std::str::from_utf8;
use quiche::h3::Priority;


const MAX_DATAGRAM_SIZE: usize = 1350;

#[tokio::main]
async fn main() -> tokio_quiche::QuicResult<()> {
    // Parse CLI parameters.
    let docopt = docopt::Docopt::new(SERVER_USAGE).unwrap();
    let conn_args = CommonArgs::with_docopt(&docopt);
    let args = ServerArgs::with_docopt(&docopt);
    let pacing = false;

    // Create the configuration for the QUIC connections.
    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();

    config.load_cert_chain_from_pem_file(&args.cert).unwrap();
    config.load_priv_key_from_pem_file(&args.key).unwrap();

    config.set_application_protos(&conn_args.alpns).unwrap();

    config.discover_pmtu(args.enable_pmtud);
    config.set_initial_rtt(conn_args.initial_rtt);
    config.set_max_idle_timeout(conn_args.idle_timeout);
    config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_initial_max_data(conn_args.max_data);
    config.set_initial_max_stream_data_bidi_local(conn_args.max_stream_data);
    config.set_initial_max_stream_data_bidi_remote(conn_args.max_stream_data);
    config.set_initial_max_stream_data_uni(conn_args.max_stream_data);
    config.set_initial_max_streams_bidi(conn_args.max_streams_bidi);
    config.set_initial_max_streams_uni(conn_args.max_streams_uni);
    config.set_disable_active_migration(!conn_args.enable_active_migration);
    config.set_active_connection_id_limit(conn_args.max_active_cids);
    config.set_initial_congestion_window_packets(
        usize::try_from(conn_args.initial_cwnd_packets).unwrap(),
    );

    config.set_max_connection_window(conn_args.max_window);
    config.set_max_stream_window(conn_args.max_stream_window);

    config.enable_pacing(pacing);

    let bind_to:String=args.listen.parse().unwrap();
    let socket = tokio::net::UdpSocket::bind(bind_to).await?;
    let mut listeners = listen(
        [socket],
        ConnectionParams::new_server(
            Default::default(),
            tokio_quiche::settings::TlsCertificatePaths {
                cert: &args.cert,
                private_key: &args.key,
                kind: tokio_quiche::settings::CertificateKind::X509,
            },
            Default::default(),
        ),
        SimpleConnectionIdGenerator,
        DefaultMetrics,
    )?;
    let accept_stream = &mut listeners[0];

    while let Some(conn) = accept_stream.next().await {
        let (driver, controller) = ServerH3Driver::new(Http3Settings::default());
        conn?.start(driver);
        tokio::spawn(handle_connection(controller));
    }
    Ok(())
}
async fn handle_connection(mut controller: ServerH3Controller) {
    while let Some(ServerH3Event::Core(event)) =
        controller.event_receiver_mut().recv().await
    {
        match event {
            H3Event::IncomingHeaders(IncomingH3Headers {
                mut send,
                headers,
                ..
            }) => {
                log::info!("incomming headers"; "headers" => ?headers);
                send.send(OutboundFrame::Headers(
                    vec![h3::Header::new(b":status", b"200")],
                    Some(Priority::new(0, false)),
                ))
                .await
                .unwrap();

                let request = &headers;
                // source: turbo-quiche
                for hdr in request {
                    match hdr.name() {
                        b":path" => {
                            
                            let path = Some(from_utf8(hdr.value()).unwrap());
                            println!("Path is: {:?}",path);
                            let body = std::fs::read(path.unwrap())
                                .unwrap_or_else(|_| b"Not Found!\r\n".to_vec());
                            send.send(OutboundFrame::body(
                                BufFactory::buf_from_slice(&body),
                                true,
                            ))
                            .await
                            .unwrap();
                        },
                        b":method" => {
                            assert_eq!(from_utf8(hdr.value()).unwrap(), "GET")
                        },
                        b":scheme" => {
                            assert_eq!(from_utf8(hdr.value()).unwrap(), "https")
                        },
                        b":authority" => {
                            //TODO
                        },
                        b"user-agent" => {
                            //ignore
                        },
                        b => {
                            println!(
                                "{} header not supported",
                                from_utf8(b).unwrap()
                            );
                        },
                    }
                }

            },
            event => {
                log::info!("event: {event:?}");
            },
        }
    }
}
