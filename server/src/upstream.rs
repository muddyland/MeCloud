//! The pooled clients that talk to Stalwart.
//!
//! Two pools with explicit timeouts: one for ordinary request/response calls,
//! and one for Server-Sent Event streams and blob downloads, which stay open
//! as long as they need to and so bound only the connect phase. Keeping them
//! apart means a burst of idle event streams cannot starve JMAP calls.

use std::time::Duration;

use reqwest::{Certificate, Client, Response};

use crate::config::Settings;
use crate::error::{AppError, Result};

#[derive(Clone)]
pub struct Upstream {
    pub client: Client,
    pub stream: Client,
}

impl Upstream {
    pub fn new(settings: &Settings) -> Upstream {
        let connect = Duration::from_secs_f64(settings.upstream_connect_timeout);
        let read = Duration::from_secs_f64(settings.upstream_read_timeout);
        let extra_roots = extra_root_certificates();
        let base = || {
            let mut builder = Client::builder()
                .user_agent(concat!("mecloud-server/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(connect)
                .pool_idle_timeout(Duration::from_secs(90));
            for cert in &extra_roots {
                builder = builder.add_root_certificate(cert.clone());
            }
            builder
        };
        let client = base()
            .read_timeout(read)
            .pool_max_idle_per_host(20)
            .build()
            .expect("the upstream HTTP client builds");
        let stream = base().build().expect("the upstream stream client builds");
        Upstream { client, stream }
    }
}

/// A private CA to trust on top of the compiled-in Mozilla roots, from
/// SSL_CERT_FILE / SSL_CERT_DIR — the same variables httpx honoured.
fn extra_root_certificates() -> Vec<Certificate> {
    let mut files = Vec::new();
    if let Ok(file) = std::env::var("SSL_CERT_FILE") {
        files.push(std::path::PathBuf::from(file));
    }
    if let Ok(dir) = std::env::var("SSL_CERT_DIR") {
        for dir in dir.split(':') {
            if let Ok(entries) = std::fs::read_dir(dir) {
                files.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_file()));
            }
        }
    }
    let mut certs = Vec::new();
    for path in files {
        match std::fs::read(&path).map(|pem| Certificate::from_pem_bundle(&pem)) {
            Ok(Ok(found)) => certs.extend(found),
            Ok(Err(e)) => tracing::warn!(
                "Ignoring {}: not a PEM certificate bundle: {e}",
                path.display()
            ),
            Err(e) => tracing::warn!("Ignoring {}: {e}", path.display()),
        }
    }
    certs
}

/// The Authorization header for an upstream call. Takes a bare OAuth access
/// token, or a credential that already names its scheme: a paired desktop
/// client authenticates with an app password over Basic.
pub fn authorization(credential: &str) -> String {
    if credential.starts_with("Bearer ") || credential.starts_with("Basic ") {
        credential.to_string()
    } else {
        format!("Bearer {credential}")
    }
}

/// An upstream error response as an [`AppError::Upstream`].
pub fn check(response: Response, what: &str) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        Ok(response)
    } else {
        Err(AppError::upstream(
            format!("{what} failed with HTTP {}", status.as_u16()),
            Some(status.as_u16()),
        ))
    }
}
