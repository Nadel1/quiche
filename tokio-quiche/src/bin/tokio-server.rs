use futures::SinkExt as _;
use futures::StreamExt as _;
use quiche::h3::NameValue;
use quiche::h3::Priority;
use regex::Regex;
use std::fs;
use std::fs::File;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::str::from_utf8;
use std::str::FromStr;
use std::time::Duration;
use tokio_quiche::args::*;
use tokio_quiche::buf_factory::BufFactory;
use tokio_quiche::http3::driver::H3Event;
use tokio_quiche::http3::driver::IncomingH3Headers;
use tokio_quiche::http3::driver::OutboundFrame;
use tokio_quiche::http3::settings::Http3Settings;
use tokio_quiche::listen;
use tokio_quiche::metrics::DefaultMetrics;
use tokio_quiche::quiche::h3;
use tokio_quiche::settings::QuicSettings;
use tokio_quiche::ConnectionParams;
use tokio_quiche::ServerH3Controller;
use tokio_quiche::ServerH3Driver;

struct MemRequest(u64);

impl FromStr for MemRequest {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let r = Regex::new(r"([0-9]+)([a-zA-Z]*)").unwrap();
        let c = r.captures(s).ok_or(())?;
        let number = c.get(1).unwrap().as_str().parse::<u64>().map_err(|_| ())?;
        let unit = c.get(2).unwrap().as_str();
        println!("str: {:?}, number: {:?}, unit: {:?}", s, number, unit);

        let number = if unit.is_empty() | unit.eq_ignore_ascii_case("B") {
            number
        } else if unit.eq_ignore_ascii_case("kB") {
            number * 1E3 as u64
        } else if unit.eq_ignore_ascii_case("MB") {
            number * 1E6 as u64
        } else if unit.eq_ignore_ascii_case("GB") {
            number * 1E9 as u64
        } else {
            return Err(());
        };
        Ok(Self(number))
    }
}

#[tokio::main]
async fn main() -> tokio_quiche::QuicResult<()> {
    let docopt: docopt::Docopt = docopt::Docopt::new(SERVER_USAGE).unwrap();
    let conn_args = CommonArgs::with_docopt(&docopt);
    let args = ServerArgs::with_docopt(&docopt);

    let bind_to: String = args.listen.parse().unwrap();

    let socket = tokio::net::UdpSocket::bind(bind_to).await?;
    let mut settings = QuicSettings::default();
    settings.logging_name = args.logging_name;
    settings.saved_params_path = args.saved_params_path;
    settings.initial_rtt = Some(Duration::from_millis(args.initial_rtt));
    settings.max_idle_timeout = Some(Duration::from_millis(args.idle_timeout));
    settings.cc_algorithm = conn_args.cc_algorithm;
    println!("Set max idle timeout: {:?}", settings.max_idle_timeout);
    println!("cert: {:?}, key: {:?}", args.cert, args.key);
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
    loop {
        if let Some(event) = controller.event_receiver_mut().recv().await {
            match event {
                tokio_quiche::http3::driver::ServerH3Event::Core(
                    H3Event::IncomingHeaders(IncomingH3Headers { .. }),
                ) => {},
                tokio_quiche::http3::driver::ServerH3Event::Headers {
                    mut incoming_headers,
                    ..
                } => {
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
                        match hdr.name() {
                            b":path" => {
                                let path = from_utf8(hdr.value());
                                let mem_request =
                                    MemRequest::from_str(path.unwrap_or("")).ok();
                                println!("path: {:?}", path);
                                let mut file =
                                    File::create(path.unwrap()).unwrap();
                                file.seek(SeekFrom::Start(
                                    mem_request.unwrap().0,
                                ))
                                .unwrap();
                                file.write_all(&[0]).unwrap();
                                let body = std::fs::read(path.unwrap())
                                    .unwrap_or_else(|_| {
                                        b"Not Found!\r\n".to_vec()
                                    });
                                incoming_headers
                                    .send
                                    .send(OutboundFrame::body(
                                        BufFactory::buf_from_slice(&body),
                                        true,
                                    ))
                                    .await
                                    .unwrap();
                                let remove = fs::remove_file(path.unwrap());
                                match remove {
                                    Ok(()) => println!(
                                        "Successfully removed generated file"
                                    ),

                                    Err(e) => {
                                        // Done writing.
                                        println!("Error while removing generated file: {:?}",e);
                                    },
                                };
                            },
                            b":method" => {
                                assert_eq!(from_utf8(hdr.value()).unwrap(), "GET")
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
                    println!("event: {event:?}");
                },
            }
        }
    }
}
