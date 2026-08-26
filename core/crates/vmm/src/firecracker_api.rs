//! A minimal HTTP/1.1 client for Firecracker's API socket. Firecracker's
//! own API surface is small, fixed-shape JSON PUT/GET requests over a Unix
//! socket — not worth pulling in a full HTTP client stack for.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

pub struct ApiClient {
    stream: BufReader<UnixStream>,
}

pub struct ApiResponse {
    pub status: u16,
    pub body: String,
}

impl ApiClient {
    pub fn connect(socket_path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(socket_path)?;
        Ok(Self { stream: BufReader::new(stream) })
    }

    pub fn put(&mut self, path: &str, json_body: &str) -> io::Result<ApiResponse> {
        let request = format!(
            "PUT {path} HTTP/1.1\r\n\
             Host: localhost\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: keep-alive\r\n\
             \r\n\
             {json_body}",
            json_body.len()
        );
        self.stream.get_mut().write_all(request.as_bytes())?;
        self.read_response()
    }

    fn read_response(&mut self) -> io::Result<ApiResponse> {
        let mut status_line = String::new();
        self.stream.read_line(&mut status_line)?;
        let status = parse_status_code(&status_line)?;

        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            self.stream.read_line(&mut line)?;
            let line = line.trim_end();
            if line.is_empty() {
                break; // end of headers
            }
            if let Some(value) = line.strip_prefix("Content-Length: ").or_else(|| line.strip_prefix("content-length: ")) {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }

        let mut body_bytes = vec![0u8; content_length];
        self.stream.read_exact(&mut body_bytes)?;
        let body = String::from_utf8_lossy(&body_bytes).into_owned();

        Ok(ApiResponse { status, body })
    }
}

fn parse_status_code(status_line: &str) -> io::Result<u16> {
    status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("malformed status line: {status_line:?}")))
}
