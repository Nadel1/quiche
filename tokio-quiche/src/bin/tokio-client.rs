use foundations::telemetry::log;
use tokio_quiche::http3::driver::ClientH3Event;
use tokio_quiche::http3::driver::H3Event;
use tokio_quiche::http3::driver::InboundFrame;
use tokio_quiche::http3::driver::IncomingH3Headers;
use tokio_quiche::quiche::h3;

#[tokio::main]
async fn main() -> tokio_quiche::QuicResult<()> {
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    socket.connect("127.0.0.1:4043").await?;

    let (_, mut controller) = tokio_quiche::quic::connect(socket, None).await?;

    controller
        .request_sender()
        .send(tokio_quiche::http3::driver::NewClientRequest {
            request_id: 0,
            headers: vec![
                h3::Header::new(b":method", b"GET"),
                h3::Header::new(b":path",b"/Cargo.toml"),
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
                            println!("{}", std::str::from_utf8(&pooled).unwrap());

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
                fin: true, ..
            }) => {
                println!("fin received");
                break;
            },
            ClientH3Event::Core(event) => println!("received event: {event:?}"),
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
    Ok(())
}
