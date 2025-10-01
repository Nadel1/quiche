use foundations::telemetry::log;
use futures::{SinkExt as _, StreamExt as _};
use quiche::h3::NameValue;
use std::str::from_utf8;
use std::time::Duration;
use tokio_quiche::args::*;
use tokio_quiche::buf_factory::BufFactory;
use tokio_quiche::http3::driver::{
    H3Event, IncomingH3Headers, OutboundFrame, ServerH3Event,
};
use tokio_quiche::http3::settings::Http3Settings;
use tokio_quiche::listen;
use tokio_quiche::metrics::DefaultMetrics;
use tokio_quiche::quic::SimpleConnectionIdGenerator;
use tokio_quiche::quiche::h3;
use tokio_quiche::settings::QuicSettings;
use tokio_quiche::{ConnectionParams, ServerH3Controller, ServerH3Driver};

use quiche::h3::Priority;

#[tokio::main]
async fn main() -> tokio_quiche::QuicResult<()> {
    // Parse CLI parameters.
    let docopt = docopt::Docopt::new(SERVER_USAGE).unwrap();
    let conn_args = CommonArgs::with_docopt(&docopt);
    let args = ServerArgs::with_docopt(&docopt);

    let bind_to: String = args.listen.parse().unwrap();
    let socket = tokio::net::UdpSocket::bind(bind_to).await?;
    let mut settings = QuicSettings::default();
    settings.max_idle_timeout =
        Some(Duration::from_millis(conn_args.idle_timeout));
    settings.initial_rtt = Some(conn_args.initial_rtt);
    settings.logging_name= conn_args.logging_name;
    settings.disable_client_ip_validation = args.no_retry;
    settings.cc_algorithm = conn_args.cc_algorithm;
    settings.initial_congestion_window_packets = conn_args.initial_cwnd_packets.try_into().unwrap();
    let mut listeners = listen(
        [socket],
        ConnectionParams::new_server(
            settings,
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
                println!("incoming");
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
                            println!("Path is: {:?}", path);
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
