//! `mecloud-server` — serve; `mecloud-server healthcheck` — probe a running
//! server on this host, for the image's HEALTHCHECK (the image has no shell,
//! no curl and no Python to do it with).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use mecloud_server::{router, warm_up, AppState, Settings};
use tracing_subscriber::EnvFilter;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(healthcheck());
    }
    if let Some(arg) = std::env::args().nth(1) {
        eprintln!("usage: mecloud-server [healthcheck]  (unknown argument {arg:?})");
        std::process::exit(2);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_ansi(false)
        .with_target(false)
        .init();

    let settings = match Settings::from_env() {
        Ok(settings) => settings,
        Err(e) => {
            eprintln!("mecloud-server: configuration error: {e}");
            std::process::exit(1);
        }
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(serve(settings));
}

async fn serve(settings: Settings) {
    let address = format!("{}:{}", settings.host, settings.port);
    let state = Arc::new(AppState::new(settings));
    if state.static_root.is_none() {
        tracing::warn!(
            "No frontend at {} — serving the API only",
            state.settings.frontend_static_dir.display()
        );
    }
    warm_up(&state).await;

    let listener = match tokio::net::TcpListener::bind(&address).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("mecloud-server: cannot listen on {address}: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!(
        "MeCloud server {} listening on {address}",
        env!("CARGO_PKG_VERSION")
    );

    let app = router(state).into_make_service_with_connect_info::<SocketAddr>();
    let server = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal());
    if let Err(e) = server.await {
        tracing::error!("server error: {e}");
        std::process::exit(1);
    }
}

/// SIGTERM or Ctrl-C, then a short grace period. Event streams never end on
/// their own, so waiting for every connection would wait for the container
/// runtime's kill instead; the clients reconnect either way.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down");
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        std::process::exit(0);
    });
}

/// GET /health on the local port; 0 when it answers 200.
fn healthcheck() -> i32 {
    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.trim().parse::<u16>().ok())
        .unwrap_or(8000);
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let attempt = || -> std::io::Result<bool> {
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
        let mut head = [0u8; 12];
        stream.read_exact(&mut head)?;
        Ok(head.starts_with(b"HTTP/1.1 200") || head.starts_with(b"HTTP/1.0 200"))
    };
    match attempt() {
        Ok(true) => 0,
        Ok(false) => {
            eprintln!("healthcheck: /health did not answer 200");
            1
        }
        Err(e) => {
            eprintln!("healthcheck: {e}");
            1
        }
    }
}
