//! Localhost benchmark transport: bounded sockets, Content-Length JSON, one request per connection.
use flate2::{write::GzEncoder, Compression};
use serde_json::json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use trip_router::model::Profile;
use trip_router::server::{Config, RouteRequest, Router};

const MAX_BODY: usize = 16 * 1024;
const MAX_HEADERS: usize = 16 * 1024;
const USAGE: &str = "trip-router-server GRAPH_ROOT PROFILE PORT WORKERS CH_CACHE_MB GEOMETRY_CACHE_MB [DEADLINE_MS]";

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(6..=7).contains(&args.len()) {
        return Err(USAGE.into());
    }
    let number = |i: usize| args[i].parse::<usize>().map_err(|e| e.to_string());
    let profile = Profile::presets().into_iter().find(|p| p.name == args[1]).ok_or("Unknown preset")?;
    let port: u16 = args[2].parse::<u16>().map_err(|e| e.to_string())?;
    let workers = number(3)?;
    if workers == 0 {
        return Err("Workers must be positive".into());
    }
    let megabytes =
        |i| -> Result<usize, String> { number(i)?.checked_mul(1024 * 1024).ok_or("Cache size overflow".into()) };
    let max_deadline = Duration::from_millis(if args.len() == 7 { number(6)? as u64 } else { 10_000 });
    let router = Arc::new(
        Router::open(Config {
            root: args[0].clone().into(),
            profile,
            ch_cache_bytes: megabytes(4)?,
            geometry_cache_bytes: megabytes(5)?,
            max_deadline,
            max_labels: 1_000_000,
            max_geometry_points: 1_000_000,
        })
        .map_err(|e| e.message)?,
    );
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    let (sender, receiver) = mpsc::sync_channel::<(TcpStream, Instant)>(workers);
    let receiver = Arc::new(Mutex::new(receiver));
    for _ in 0..workers {
        let receiver = receiver.clone();
        let router = router.clone();
        std::thread::spawn(move || loop {
            let Ok((stream, accepted)) = receiver.lock().unwrap().recv() else { break };
            handle(stream, accepted, &router);
        });
    }
    eprintln!(
        "{}",
        json!({"listening":listener.local_addr().map_err(|e| e.to_string())?.to_string(),
        "profile":args[1],"workers":workers,"queued_sockets":workers,"max_deadline_ms":max_deadline.as_millis(),
        "ch_cache_bytes":router.config.ch_cache_bytes,"geometry_cache_bytes":router.config.geometry_cache_bytes,
        "cache_eviction":false,"endpoint":"POST /route","endpoint_semantics":"explicit graph states; incoming seed road omitted"})
    );
    for connection in listener.incoming() {
        let stream = connection.map_err(|e| e.to_string())?;
        if let Err(mpsc::TrySendError::Full((mut stream, _))) = sender.try_send((stream, Instant::now())) {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
            let _ = respond(&mut stream, 503, &json!({"kind":"busy","message":"Worker queue is full"}), false);
        }
    }
    Ok(())
}

fn handle(mut stream: TcpStream, accepted: Instant, router: &Router) {
    let deadline = accepted + router.config.max_deadline;
    let parsed = read_request(&mut stream, deadline);
    let (mut request, gzip) = match parsed {
        Ok(value) => value,
        Err((status, message)) => {
            let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
            let _ = respond(&mut stream, status, &json!({"kind":"invalid_request","message":message}), false);
            return;
        }
    };
    let remaining = deadline.saturating_duration_since(Instant::now()).as_millis() as u64;
    // Queue and body time count towards the process's maximum deadline.
    if request.deadline_ms.is_none() {
        request.deadline_ms = Some(remaining);
    } else if request.deadline_ms.is_some_and(|ms| ms <= router.config.max_deadline.as_millis() as u64) {
        request.deadline_ms = request.deadline_ms.map(|ms| ms.min(remaining));
    }
    let _ = stream.set_nonblocking(true);
    let result = router.route(&request, || match stream.peek(&mut [0]) {
        Ok(0) => true,
        Err(e) => e.kind() != std::io::ErrorKind::WouldBlock,
        _ => false,
    });
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    match result {
        Ok(route) => {
            let _ = respond(&mut stream, 200, &route, gzip);
        }
        Err(error) => {
            let status = match error.kind {
                "invalid" => 400,
                "no_path" => 404,
                "limit" => 503,
                "deadline" => 504,
                "cancelled" => 499,
                _ => 500,
            };
            let _ = respond(&mut stream, status, &error, false);
        }
    }
}

type HttpResult<T> = Result<T, (u16, String)>;

fn read_request(stream: &mut TcpStream, deadline: Instant) -> HttpResult<(RouteRequest, bool)> {
    let mut bytes = Vec::new();
    let (header_len, body_len, gzip) = loop {
        let mut headers = [httparse::EMPTY_HEADER; 32];
        let mut request = httparse::Request::new(&mut headers);
        match request.parse(&bytes).map_err(|e| (400, e.to_string()))? {
            httparse::Status::Complete(offset) => {
                if offset > MAX_HEADERS {
                    return Err((413, "Request headers exceed budget".into()));
                }
                if request.method != Some("POST") || request.path != Some("/route") {
                    return Err((404, "Use POST /route".into()));
                }
                let mut length = None;
                let mut gzip = false;
                for header in request.headers.iter() {
                    if header.name.eq_ignore_ascii_case("content-length") {
                        if length.is_some() {
                            return Err((400, "Duplicate Content-Length".into()));
                        }
                        length = Some(
                            std::str::from_utf8(header.value)
                                .ok()
                                .and_then(|v| v.parse::<usize>().ok())
                                .ok_or_else(|| (400, "Invalid Content-Length".into()))?,
                        );
                    }
                    if header.name.eq_ignore_ascii_case("transfer-encoding")
                        || header.name.eq_ignore_ascii_case("expect")
                    {
                        return Err((400, "Transfer-Encoding and Expect are unsupported in this harness".into()));
                    }
                    if header.name.eq_ignore_ascii_case("accept-encoding") {
                        gzip = std::str::from_utf8(header.value)
                            .unwrap_or("")
                            .split(',')
                            .any(|part| part.trim() == "gzip");
                    }
                }
                let length = length.ok_or_else(|| (411, "Content-Length required".into()))?;
                if length > MAX_BODY {
                    return Err((413, "Request body exceeds budget".into()));
                }
                break (offset, length, gzip);
            }
            httparse::Status::Partial => {
                if bytes.len() >= MAX_HEADERS {
                    return Err((413, "Request headers exceed budget".into()));
                }
                read_chunk(stream, deadline, &mut bytes, 1024)?;
            }
        }
    };
    while bytes.len() < header_len + body_len {
        let remaining = header_len + body_len - bytes.len();
        read_chunk(stream, deadline, &mut bytes, remaining.min(1024))?;
    }
    let request =
        serde_json::from_slice(&bytes[header_len..header_len + body_len]).map_err(|e| (400, e.to_string()))?;
    Ok((request, gzip))
}

fn read_chunk(stream: &mut TcpStream, deadline: Instant, bytes: &mut Vec<u8>, limit: usize) -> HttpResult<()> {
    let remaining =
        deadline.checked_duration_since(Instant::now()).ok_or_else(|| (408, "Request deadline exceeded".into()))?;
    stream.set_read_timeout(Some(remaining)).map_err(|e| (400, e.to_string()))?;
    let mut chunk = [0; 1024];
    let n = stream.read(&mut chunk[..limit]).map_err(|e| (408, e.to_string()))?;
    if n == 0 {
        return Err((400, "Incomplete request".into()));
    }
    bytes.extend_from_slice(&chunk[..n]);
    Ok(())
}

fn respond(stream: &mut TcpStream, status: u16, value: &impl serde::Serialize, gzip: bool) -> std::io::Result<()> {
    let started = Instant::now();
    let raw = serde_json::to_vec(value)?;
    let serialize_ms = started.elapsed().as_secs_f64() * 1000.0;
    let compress_started = Instant::now();
    let bytes = if gzip {
        let mut writer = GzEncoder::new(Vec::new(), Compression::fast());
        writer.write_all(&raw)?;
        writer.finish()?
    } else {
        raw
    };
    let compress_ms = compress_started.elapsed().as_secs_f64() * 1000.0;
    write!(stream, "HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nServer-Timing: serialize;dur={serialize_ms:.3}, compress;dur={compress_ms:.3}\r\n{}\r\n",
        bytes.len(), if gzip { "Content-Encoding: gzip\r\n" } else { "" })?;
    stream.write_all(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    #[test]
    fn bounded_transport_parses_content_length_and_rejects_oversized_body() {
        let body = br#"{"start":{"node":0,"road":0,"cost":0},"end":{"node":1,"road":1,"cost":0}}"#;
        let (mut client, mut server) = connection();
        write!(client, "POST /route HTTP/1.1\r\nContent-Length: {}\r\nAccept-Encoding: gzip\r\n\r\n", body.len())
            .unwrap();
        client.write_all(body).unwrap();
        let (request, gzip) = read_request(&mut server, Instant::now() + Duration::from_secs(1)).unwrap();
        assert_eq!(request.end.node, 1);
        assert!(gzip);
        let (mut client, mut server) = connection();
        write!(client, "POST /route HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1).unwrap();
        assert!(matches!(read_request(&mut server, Instant::now() + Duration::from_secs(1)), Err((413, _))));
        let (_, mut server) = connection();
        assert!(matches!(read_request(&mut server, Instant::now()), Err((408, _))));
    }

    #[test]
    fn gzip_response_roundtrips_and_reports_serialization_cost() {
        let (mut client, mut server) = connection();
        let expected = json!({"kind":"done","roads":[1,2,3]});
        respond(&mut server, 200, &expected, true).unwrap();
        drop(server);
        let mut bytes = Vec::new();
        client.read_to_end(&mut bytes).unwrap();
        let mut headers = [httparse::EMPTY_HEADER; 16];
        let mut response = httparse::Response::new(&mut headers);
        let httparse::Status::Complete(offset) = response.parse(&bytes).unwrap() else {
            panic!("Incomplete HTTP response")
        };
        assert_eq!(response.code, Some(200));
        assert!(response.headers.iter().any(|h| h.name.eq_ignore_ascii_case("content-encoding") && h.value == b"gzip"));
        assert!(response.headers.iter().any(|h| h.name.eq_ignore_ascii_case("server-timing")));
        let decoder = flate2::read::GzDecoder::new(&bytes[offset..]);
        assert_eq!(serde_json::from_reader::<_, serde_json::Value>(decoder).unwrap(), expected);
    }
}
