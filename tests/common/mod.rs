//! Shared helpers for integration tests.
#![allow(dead_code)]

use base64::Engine;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;

/// Starts a local fake OAuth token endpoint and returns its URL. It answers
/// based on the refresh_token it receives:
/// - `fresh:<email>` → 200 with an id_token for <email>
/// - `stale`         → 400 invalid_grant / invalid_rapt
/// - anything else   → 400 invalid_grant
pub fn start_fake_token_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let l = line.trim_end().to_ascii_lowercase();
                if let Some(v) = l.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
                if l.is_empty() {
                    break;
                }
            }
            let mut body = vec![0u8; len];
            let _ = reader.read_exact(&mut body);
            let body = String::from_utf8_lossy(&body).to_string();
            let token = body
                .split('&')
                .find_map(|kv| kv.strip_prefix("refresh_token="))
                .unwrap_or("")
                .replace("%3A", ":")
                .replace("%40", "@");
            let (status, json) = if let Some(email) = token.strip_prefix("fresh:") {
                let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
                let jwt = format!(
                    "{}.{}.sig",
                    enc.encode("{}"),
                    enc.encode(format!(r#"{{"email":"{}"}}"#, email))
                );
                (
                    "200 OK",
                    format!(r#"{{"access_token":"x","id_token":"{}"}}"#, jwt),
                )
            } else if token == "stale" {
                (
                    "400 Bad Request",
                    r#"{"error":"invalid_grant","error_subtype":"invalid_rapt"}"#.to_string(),
                )
            } else {
                (
                    "400 Bad Request",
                    r#"{"error":"invalid_grant"}"#.to_string(),
                )
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status,
                json.len(),
                json
            );
        }
    });
    format!("http://{}/token", addr)
}
