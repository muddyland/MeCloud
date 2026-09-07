//! Getting a credential for the sync engine.
//!
//! The webview's session belongs to the webview: it is an HttpOnly cookie in a
//! browser jar, and a background thread cannot borrow it. So the client asks
//! the server for a device token, through the user.
//!
//! The exchange is the loopback redirect that native OAuth clients use. We
//! listen on a port on this machine, send the user to a page on the server that
//! requires their session and asks them to approve, and the server redirects
//! the answer back to that port. The token therefore never passes through a
//! process that has not been approved, and never appears on a network.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

/// How long to wait for the user to approve. Long enough to read the screen
/// and sign in first if the browser was not already signed in.
const WAIT: Duration = Duration::from_secs(180);

pub struct Listener {
    listener: TcpListener,
    pub port: u16,
}

impl Listener {
    /// Bind a port on the loopback interface only.
    pub fn bind() -> Result<Self, String> {
        // Port 0: the operating system picks a free one, which avoids both a
        // fixed port that could already be taken and a guess that another
        // process could squat on ahead of us.
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let listener = TcpListener::bind(addr).map_err(|e| format!("Could not open a local port: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener
            .set_nonblocking(false)
            .map_err(|e| e.to_string())?;
        Ok(Self { listener, port })
    }

    pub fn redirect_url(&self) -> String {
        format!("http://127.0.0.1:{}/paired", self.port)
    }

    /// The URL to send the user to.
    pub fn pairing_url(&self, server: &str, device_name: &str) -> String {
        format!(
            "{}/auth/device?redirect={}&name={}",
            server.trim_end_matches('/'),
            encode(&self.redirect_url()),
            encode(device_name)
        )
    }

    /// Wait for the server to redirect the browser back with a token.
    pub fn wait_for_token(self) -> Result<String, String> {
        self.listener
            .set_nonblocking(false)
            .map_err(|e| e.to_string())?;

        let deadline = std::time::Instant::now() + WAIT;
        for stream in self.listener.incoming() {
            if std::time::Instant::now() > deadline {
                return Err("Timed out waiting for approval.".into());
            }
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));

            let Some(target) = read_request_target(&mut stream) else {
                respond(&mut stream, "Could not read that request.");
                continue;
            };
            match token_from_target(&target) {
                Some(token) => {
                    respond(&mut stream, "Connected. You can close this window and go back to MeCloud.");
                    return Ok(token);
                }
                None => {
                    // A browser fetching /favicon.ico on the way is normal; keep
                    // listening rather than failing the pairing over it.
                    respond(&mut stream, "Waiting for approval…");
                }
            }
        }
        Err("The local listener closed before approval arrived.".into())
    }
}

/// The request target from an HTTP request line, e.g. "/paired?token=abc".
fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    if method != "GET" {
        return None;
    }
    parts.next().map(str::to_string)
}

/// Pull the token out of the redirect the server sent the browser to.
pub fn token_from_target(target: &str) -> Option<String> {
    let (path, query) = target.split_once('?')?;
    if path != "/paired" {
        return None;
    }
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("token=") {
            let decoded = decode(value);
            if !decoded.is_empty() {
                return Some(decoded);
            }
        }
    }
    None
}

fn respond(stream: &mut TcpStream, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>MeCloud</title>\
         <body style=\"font:16px system-ui;display:grid;place-items:center;height:100vh;margin:0\">\
         <p>{message}</p></body>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_a_loopback_port_and_describes_itself() {
        let listener = Listener::bind().unwrap();
        assert!(listener.port >= 1024);
        assert_eq!(
            listener.redirect_url(),
            format!("http://127.0.0.1:{}/paired", listener.port)
        );
    }

    #[test]
    fn the_pairing_url_carries_an_encoded_loopback_redirect() {
        let listener = Listener::bind().unwrap();
        let url = listener.pairing_url("https://cloud.example.com/", "Ada's laptop");
        assert!(url.starts_with("https://cloud.example.com/auth/device?redirect="));
        assert!(url.contains("http%3A%2F%2F127.0.0.1%3A"));
        assert!(url.contains("name=Ada%27s%20laptop"));
        // The server refuses a non-loopback redirect, so this must never
        // become one by accident.
        assert!(!url.contains("localhost"));
    }

    #[test]
    fn reads_the_token_out_of_the_redirect() {
        assert_eq!(token_from_target("/paired?token=abc123").as_deref(), Some("abc123"));
        assert_eq!(
            token_from_target("/paired?state=x&token=a%2Bb%3Dc").as_deref(),
            Some("a+b=c")
        );
    }

    #[test]
    fn ignores_requests_that_are_not_the_redirect() {
        // A browser asks for /favicon.ico on the way; that must not abort it.
        assert_eq!(token_from_target("/favicon.ico"), None);
        assert_eq!(token_from_target("/paired"), None);
        assert_eq!(token_from_target("/paired?token="), None);
        assert_eq!(token_from_target("/other?token=abc"), None);
        assert_eq!(token_from_target(""), None);
    }

    #[test]
    fn percent_decoding_round_trips_what_we_encode() {
        for value in ["plain", "a b", "a/b?c=d&e", "Ada's laptop", "unicode ✓"] {
            assert_eq!(decode(&encode(value)), value);
        }
    }
}
