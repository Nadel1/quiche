use foundations::telemetry::log;
use futures::{SinkExt as _, StreamExt as _};
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

use quiche::h3::Priority;
#[tokio::main]
async fn main(){
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:4043").await?;
    let mut listeners = listen(
        [socket],
        ConnectionParams::new_server(
            Default::default(),
            tokio_quiche::settings::TlsCertificatePaths {
                cert: "/path/to/cert.pem",
                private_key: "/path/to/key.pem",
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
                send.send(OutboundFrame::Headers(vec![h3::Header::new(
                    b":status", b"200", 
                )],Some(Priority::new(3, 1))))
                .await
                .unwrap();

                send.send(OutboundFrame::body(
                    BufFactory::buf_from_slice(b"hello from TQ!"),
                    true,
                ),None)
                .await
                .unwrap();
            },
            event => {
                log::info!("event: {event:?}");
            },
        }
    }
}
