//! A loopback stand-in for Discord's REST API, for tests of the code that
//! talks to it through serenity: one canned answer per route, one request
//! per connection, and a record of what was asked.

#![allow(clippy::unwrap_used, clippy::panic, reason = "tests may panic")]

use std::sync::{Arc, Mutex};

use poise::serenity_prelude as serenity;

/// The stand-in, listening until it is dropped.
pub struct Stub {
    /// Where it listens: what [`http`] is pointed at.
    pub base: String,
    /// Every request so far, as `(request line, body)`.
    pub requests: Arc<Mutex<Vec<(String, String)>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// `answers` are `(start of the request line, status, JSON body)`. A request
/// none of them fits gets a 404.
pub async fn stub(answers: &'static [(&'static str, u16, &'static str)]) -> Stub {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    let server = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let (line, body) = read_request(&stream).await;
            let (status, answer) = answers
                .iter()
                .find(|(start, ..)| line.starts_with(start))
                .map_or((404, "{}"), |(_, status, answer)| (*status, *answer));
            seen.lock().unwrap().push((line, body));
            let response = format!(
                "HTTP/1.1 {status} Stub\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{answer}",
                answer.len()
            );
            write_all(&stream, response.as_bytes()).await;
        }
    });
    Stub {
        base,
        requests,
        server,
    }
}

/// A serenity client that sends its requests to `base` instead of Discord.
pub fn http(base: &str) -> serenity::Http {
    serenity::HttpBuilder::new("test-token")
        .proxy(base)
        .ratelimiter_disabled(true)
        .build()
}

/// Reads one HTTP request; returns its request line and body.
async fn read_request(stream: &tokio::net::TcpStream) -> (String, String) {
    let mut bytes = Vec::new();
    loop {
        stream.readable().await.unwrap();
        let mut chunk = [0_u8; 4096];
        match stream.try_read(&mut chunk) {
            Ok(0) => panic!("the request ended early"),
            Ok(n) => bytes.extend(chunk.iter().take(n)),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => panic!("reading the request: {e}"),
        }
        let text = String::from_utf8_lossy(&bytes);
        let Some((head, body)) = text.split_once("\r\n\r\n") else {
            continue;
        };
        let length = head
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map_or(0, |(_, value)| value.trim().parse::<usize>().unwrap());
        if body.len() >= length {
            return (head.lines().next().unwrap().to_owned(), body.to_owned());
        }
    }
}

async fn write_all(stream: &tokio::net::TcpStream, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        stream.writable().await.unwrap();
        match stream.try_write(bytes) {
            Ok(n) => bytes = bytes.get(n..).unwrap(),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("writing the response: {e}"),
        }
    }
}
