//! Getting a credential for the sync engine.
//!
//! The webview's session belongs to the webview: it is an HttpOnly cookie in a
//! browser jar, and a background thread cannot borrow it. So the client asks
//! the server for a device token, through the user.
//!
//! The exchange is the loopback flow native OAuth clients use. We listen on a
//! port on this machine, send the user to a page on the server that requires
//! their session and asks them to approve, and the server hands the answer back
//! to that port. The token never appears on a network.
//!
//! Two details carry the security of it:
//!
//! - **A state nonce.** The client makes one up, sends it to the server with
//!   the request, and accepts a token only if the same value comes back with
//!   it. Anything else that can reach the port while it is open — including a
//!   web page in the user's browser probing localhost — cannot know it, so it
//!   cannot substitute a token of its own.
//! - **A form POST, not a redirect.** The token arrives in a request body, not
//!   a URL, so it never lands in browser history (or in history synced to
//!   another machine). A GET carrying `?token=` — the old protocol — is refused.
//!
//! The wait is bounded and can be cancelled, so closing the browser tab does
//! not leave a thread and a port held for the life of the process.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::encode;

/// How long to wait for the user to approve. Long enough to read the screen
/// and sign in first if the browser was not already signed in.
pub const WAIT: Duration = Duration::from_secs(180);

/// The largest request body accepted. A token and a nonce, form-encoded.
const MAX_BODY: usize = 16 * 1024;

pub struct Listener {
    listener: TcpListener,
    pub port: u16,
    state: String,
}

/// What one request to the listener amounted to.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The approval, with the token.
    Token(String),
    /// Something else — a favicon, a stray probe, a wrong nonce — answered
    /// with this message, and the wait goes on.
    KeepWaiting(&'static str),
}

/// One parsed HTTP request: just what the listener needs.
#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub body: String,
}

impl Listener {
    /// Bind a port on the loopback interface only, and pick a nonce.
    pub fn bind() -> Result<Self, String> {
        // Port 0: the operating system picks a free one, which avoids both a
        // fixed port that could already be taken and a guess that another
        // process could squat on ahead of us.
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let listener = TcpListener::bind(addr).map_err(|e| format!("Could not open a local port: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut nonce).map_err(|e| format!("No randomness available: {e}"))?;
        Ok(Self { listener, port, state: hex::encode(nonce) })
    }

    pub fn redirect_url(&self) -> String {
        format!("http://127.0.0.1:{}/paired", self.port)
    }

    /// The URL to send the user to.
    pub fn pairing_url(&self, server: &str, device_name: &str) -> String {
        format!(
            "{}/auth/device?redirect={}&name={}&state={}",
            server.trim_end_matches('/'),
            encode::component(&self.redirect_url()),
            encode::component(device_name),
            self.state,
        )
    }

    /// Wait for the approval, until `limit` passes or `cancel` is set.
    pub fn wait_for_token(self, limit: Duration, cancel: &AtomicBool) -> Result<String, String> {
        // Non-blocking, polled: a blocking accept has no deadline, and the old
        // listener sat in one forever if the tab was closed.
        self.listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + limit;

        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Connecting was cancelled.".into());
            }
            if Instant::now() >= deadline {
                return Err("Timed out waiting for approval. Try again when you are ready.".into());
            }
            let mut stream = match self.listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

            let outcome = match read_request(&mut stream) {
                Some(request) => self.judge(&request),
                None => Outcome::KeepWaiting("Could not read that request."),
            };
            match outcome {
                Outcome::Token(token) => {
                    respond(&mut stream, "Connected. You can close this window and go back to MeCloud.");
                    return Ok(token);
                }
                Outcome::KeepWaiting(message) => respond(&mut stream, message),
            }
        }
    }

    /// Decide what one request means.
    pub fn judge(&self, request: &Request) -> Outcome {
        if request.path != "/paired" {
            // A browser asks for /favicon.ico on the way; that is not a failure.
            return Outcome::KeepWaiting("Waiting for approval…");
        }
        if request.method != "POST" {
            return Outcome::KeepWaiting(
                "This server sent the approval in a form this version of MeCloud no longer \
                 accepts. Update the server, then connect again.",
            );
        }
        let mut token = None;
        let mut state = None;
        for (key, value) in url::form_urlencoded::parse(request.body.as_bytes()) {
            match key.as_ref() {
                "token" => token = Some(value.into_owned()),
                "state" => state = Some(value.into_owned()),
                _ => {}
            }
        }
        let state_ok = state.as_deref().is_some_and(|s| constant_time_eq(s.as_bytes(), self.state.as_bytes()));
        if !state_ok {
            return Outcome::KeepWaiting(
                "That approval was for a different request. Start again from MeCloud.",
            );
        }
        match token.filter(|t| !t.trim().is_empty()) {
            Some(t) => Outcome::Token(t.trim().to_string()),
            None => Outcome::KeepWaiting("The approval arrived without a token."),
        }
    }

    #[cfg(test)]
    pub fn state(&self) -> &str {
        &self.state
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Read a request line, headers and a bounded body.
pub fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?;
    let path = target.split('?').next().unwrap_or("").to_string();

    let mut length = 0usize;
    for _ in 0..100 {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().ok()?;
            }
        }
    }
    if length > MAX_BODY {
        return None;
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    Some(Request { method, path, body: String::from_utf8_lossy(&body).into_owned() })
}

fn respond(stream: &mut TcpStream, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>MeCloud</title>\
         <body style=\"font:16px system-ui;display:grid;place-items:center;height:100vh;margin:0\">\
         <p>{message}</p></body>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(body: &str) -> Request {
        Request { method: "POST".into(), path: "/paired".into(), body: body.into() }
    }

    #[test]
    fn binds_a_loopback_port_and_describes_itself() {
        let listener = Listener::bind().unwrap();
        assert!(listener.port >= 1024);
        assert_eq!(listener.redirect_url(), format!("http://127.0.0.1:{}/paired", listener.port));
    }

    #[test]
    fn the_pairing_url_carries_an_encoded_loopback_redirect_and_the_state() {
        let listener = Listener::bind().unwrap();
        let url = listener.pairing_url("https://cloud.example.com/", "Ada's laptop");
        assert!(url.starts_with("https://cloud.example.com/auth/device?redirect="));
        assert!(url.contains("http%3A%2F%2F127.0.0.1%3A"));
        assert!(url.contains("name=Ada%27s%20laptop"));
        assert!(url.contains(&format!("&state={}", listener.state())));
        // The server refuses a non-loopback redirect, so this must never
        // become one by accident.
        assert!(!url.contains("localhost"));
    }

    #[test]
    fn the_state_fits_what_the_server_accepts() {
        // 16-128 characters of [A-Za-z0-9_-].
        let listener = Listener::bind().unwrap();
        let state = listener.state();
        assert!((16..=128).contains(&state.len()));
        assert!(state.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        assert_ne!(state, Listener::bind().unwrap().state(), "each request needs its own nonce");
    }

    #[test]
    fn a_post_with_the_right_state_is_the_token() {
        let listener = Listener::bind().unwrap();
        let body = format!("token=a%2Bb%3Dc&state={}", listener.state());
        assert_eq!(listener.judge(&post(&body)), Outcome::Token("a+b=c".into()));
    }

    #[test]
    fn a_wrong_or_missing_state_is_refused_and_the_wait_goes_on() {
        let listener = Listener::bind().unwrap();
        for body in ["token=abc&state=guess", "token=abc", "token=abc&state="] {
            assert!(matches!(listener.judge(&post(body)), Outcome::KeepWaiting(_)), "{body}");
        }
    }

    #[test]
    fn the_old_get_redirect_is_refused() {
        // A token in a URL ends up in browser history.
        let listener = Listener::bind().unwrap();
        let get = Request { method: "GET".into(), path: "/paired".into(), body: String::new() };
        assert!(matches!(listener.judge(&get), Outcome::KeepWaiting(_)));
    }

    #[test]
    fn requests_that_are_not_the_approval_are_ignored() {
        let listener = Listener::bind().unwrap();
        let favicon = Request { method: "GET".into(), path: "/favicon.ico".into(), body: String::new() };
        assert!(matches!(listener.judge(&favicon), Outcome::KeepWaiting(_)));
        let body = format!("token=&state={}", listener.state());
        assert!(matches!(listener.judge(&post(&body)), Outcome::KeepWaiting(_)));
    }

    #[test]
    fn the_wait_ends_at_its_deadline_and_frees_the_port() {
        let listener = Listener::bind().unwrap();
        let port = listener.port;
        let started = Instant::now();
        let result = listener.wait_for_token(Duration::from_millis(300), &AtomicBool::new(false));
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
        // The listener was consumed and dropped, so the port is free again.
        assert!(TcpListener::bind(("127.0.0.1", port)).is_ok());
    }

    #[test]
    fn the_wait_can_be_cancelled() {
        let listener = Listener::bind().unwrap();
        let cancel = AtomicBool::new(true);
        let result = listener.wait_for_token(Duration::from_secs(60), &cancel);
        assert_eq!(result.unwrap_err(), "Connecting was cancelled.");
    }

    #[test]
    fn a_real_form_post_over_tcp_is_accepted() {
        let listener = Listener::bind().unwrap();
        let port = listener.port;
        let body = format!("token=tok-123&state={}", listener.state());
        let sender = std::thread::spawn(move || {
            // A stray probe first, which must not end the wait.
            let mut probe = TcpStream::connect(("127.0.0.1", port)).unwrap();
            probe.write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let mut ignored = String::new();
            let _ = probe.read_to_string(&mut ignored);

            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let request = format!(
                "POST /paired HTTP/1.1\r\nHost: 127.0.0.1\r\n\
                 Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(request.as_bytes()).unwrap();
            let mut reply = String::new();
            let _ = stream.read_to_string(&mut reply);
            reply
        });
        let token = listener.wait_for_token(Duration::from_secs(10), &AtomicBool::new(false)).unwrap();
        assert_eq!(token, "tok-123");
        assert!(sender.join().unwrap().contains("Connected"));
    }
}
