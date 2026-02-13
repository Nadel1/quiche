use foundations::telemetry::log;
use futures::SinkExt as _;
use futures::StreamExt as _;
use quiche::h3::NameValue;
use quiche::h3::Priority;
use std::str::from_utf8;
use tokio_quiche::buf_factory::BufFactory;
use tokio_quiche::http3::driver::H3Event;
use tokio_quiche::http3::driver::IncomingH3Headers;
use tokio_quiche::http3::driver::OutboundFrame;
use tokio_quiche::http3::driver::ServerH3Event;
use tokio_quiche::http3::settings::Http3Settings;
use tokio_quiche::listen;
use tokio_quiche::metrics::DefaultMetrics;
use tokio_quiche::quic::SimpleConnectionIdGenerator;
use tokio_quiche::quiche::h3;
use tokio_quiche::ConnectionParams;
use tokio_quiche::ServerH3Controller;
use tokio_quiche::ServerH3Driver;

#[tokio::main]
async fn main() -> tokio_quiche::QuicResult<()> {
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:4433").await?;

    let mut listeners = listen(
        [socket],
        ConnectionParams::new_server(
            Default::default(),
            tokio_quiche::settings::TlsCertificatePaths {
                cert: "tokio-quiche/src/bin/cert.crt",
                private_key: "tokio-quiche/src/bin/cert.key",
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
    // while let Some(conn_res) = accept_stream.next().await {
    //    match conn_res {
    //        Ok(conn) => {
    //            log::info!("received new connection!");
    //
    //            // Create an `H3Driver` to serve the connection.
    //            let (driver, controller) =
    //                ServerH3Driver::new(Http3Settings::default());
    //
    //            // Start the driver. This will execute the handshake under the
    //            // hood, which lets us start receiving
    //            // ServerH3Events without needing to do
    //            // anything extra after this future resolves.
    //            conn.start(driver);
    //
    //            // Spawn a task to process the new connection.
    //            tokio::spawn(handle_connection(controller));
    //        },
    //        Err(e) => {
    //            log::error!("could not create connection: {e:?}");
    //        },
    //    }
    //}

    Ok(())
}

async fn handle_connection(mut controller: ServerH3Controller) {
    loop {
        match controller.event_receiver_mut().recv().await {
            Some(event) => {
                println!("incoming event in server bin: {:?}", event);
                match event {
                    tokio_quiche::http3::driver::ServerH3Event::Core(
                        H3Event::IncomingHeaders(IncomingH3Headers {
                            mut send,
                            headers,
                            ..
                        }),
                    ) => {
                        println!("---this is not entered----")
                    },
                    tokio_quiche::http3::driver::ServerH3Event::Headers {
                        mut incoming_headers,
                        priority,
                        is_in_early_data,
                    } => {
                        println!("headers!!!!");
                        println!("---incoming----");
                        log::info!("incomming headers"; "headers" => ?incoming_headers);
                        incoming_headers
                            .send
                            .send(OutboundFrame::Headers(
                                vec![h3::Header::new(b":status", b"200")],
                                Some(Priority::new(0, true)),
                            ))
                            .await
                            .unwrap();
                        let request = &incoming_headers.headers;
                        for hdr in request {
                            println!("header: {hdr:?}");
                            match hdr.name() {
                                b":path" => {
                                    let path =
                                        Some(from_utf8(hdr.value()).unwrap());
                                    println!("Path is: {:?}", path);
                                    let body = std::fs::read(path.unwrap())
                                        .unwrap_or_else(|_| {
                                            b"Not Found!\r\n"
                                                .to_vec()
                                        });
                                    incoming_headers.send.send(OutboundFrame::body(
                                        BufFactory::buf_from_slice(&body),
                                        true,
                                    ))
                                    .await
                                    .unwrap();
                                },
                                b":method" => {
                                    assert_eq!(
                                        from_utf8(hdr.value()).unwrap(),
                                        "GET"
                                    )
                                },
                                b":scheme" => {
                                    assert_eq!(
                                        from_utf8(hdr.value()).unwrap(),
                                        "http"
                                    )
                                },
                                b":authority" => {
                                    // TODO
                                },
                                b"user-agent" => {
                                    // ignore
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
                        println!("---do not enter---event: {event:?}");
                    },
                }
            },
            None => (),
        }
    }
    // while let Some(ServerH3Event::Core(event)) =
    //    controller.event_receiver_mut().recv().await
    //{
    //    println!("incoming event in server bin: {:?}", event);
    //    match event {
    //        H3Event::IncomingHeaders(IncomingH3Headers {
    //            mut send,
    //            headers,
    //            ..
    //        }) => {
    //            println!("incoming");
    //            log::info!("incomming headers"; "headers" => ?headers);
    //            send.send(OutboundFrame::Headers(
    //                vec![h3::Header::new(b":status", b"200")],
    //                Some(Priority::new(0, true)),
    //            ))
    //            .await
    //            .unwrap();
    //
    //            let request = &headers;
    //
    //            for hdr in request {
    //                println!("header: {hdr:?}");
    //                match hdr.name() {
    //                    b":path" => {
    //                        let path = Some(from_utf8(hdr.value()).unwrap());
    //                        println!("Path is: {:?}", path);
    //                        let body = std::fs::read(path.unwrap())
    //                            .unwrap_or_else(|_| b"Not
    // Found!\r\n".to_vec());
    // send.send(OutboundFrame::body(
    // BufFactory::buf_from_slice(&body),                            true,
    //                        ))
    //                        .await
    //                        .unwrap();
    //                    },
    //                    b":method" => {
    //                        assert_eq!(from_utf8(hdr.value()).unwrap(), "GET")
    //                    },
    //                    b":scheme" => {
    //                        assert_eq!(from_utf8(hdr.value()).unwrap(),
    // "http")                    },
    //                    b":authority" => {
    //                        // TODO
    //                    },
    //                    b"user-agent" => {
    //                        // ignore
    //                    },
    //                    b => {
    //                        println!(
    //                            "{} header not supported",
    //                            from_utf8(b).unwrap()
    //                        );
    //                    },
    //                }
    //            }
    //        },
    //        event => {
    //            println!("event: {event:?}");
    //        },
    //    }
}
