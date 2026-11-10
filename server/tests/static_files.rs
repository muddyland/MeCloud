//! Serving the built frontend: files, the SPA fallback, and its boundaries.

mod common;

use axum::body::Body;
use axum::http::Request;
use base64::Engine as _;
use common::*;

const BOOT: &str = "\n  __sveltekit_boot();\n";

fn build_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mecloud-static-{}",
        mecloud_server::crypto::sha256_hex(&mecloud_server::crypto::random_bytes::<8>())
    ));
    std::fs::create_dir_all(dir.join("_app/immutable/entry")).unwrap();
    std::fs::write(
        dir.join("index.html"),
        format!(r#"<!doctype html><html><head><meta charset="utf-8"><script type="module" src="/_app/immutable/entry/start.js"></script></head><body><script>{BOOT}</script></body></html>"#),
    )
    .unwrap();
    std::fs::write(
        dir.join("_app/immutable/entry/start.js"),
        "export const x = 1;",
    )
    .unwrap();
    std::fs::write(dir.join("robots.txt"), "User-agent: *").unwrap();
    std::fs::write(dir.join("manifest.json"), "{}").unwrap();
    dir
}

async fn app_with_frontend() -> (TestApp, std::path::PathBuf) {
    let dir = build_dir();
    let app = TestApp::with(&[("FRONTEND_STATIC_DIR", dir.to_str().unwrap())]).await;
    (app, dir)
}

#[tokio::test]
async fn files_are_served_with_the_right_caching() {
    let (app, dir) = app_with_frontend().await;
    let r = app.get("/_app/immutable/entry/start.js").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.text(), "export const x = 1;");
    assert_eq!(
        r.header("cache-control"),
        "public, max-age=31536000, immutable"
    );
    assert!(r.header("content-type").contains("javascript"));
    let r = app.get("/robots.txt").await;
    assert_eq!(
        r.header("cache-control"),
        "public, max-age=0, must-revalidate"
    );
    assert_eq!(r.header("content-type"), "text/plain; charset=utf-8");
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn app_routes_get_the_shell_which_always_revalidates() {
    let (app, dir) = app_with_frontend().await;
    for path in [
        "/",
        "/calendar",
        "/files/some/deep/path",
        "/apis-and-things",
        "/authors",
    ] {
        let r = app.get(path).await;
        assert_eq!(r.status, 200, "{path}");
        assert!(r.text().contains("__sveltekit_boot"), "{path}");
        assert_eq!(r.header("cache-control"), "no-cache");
        assert!(r.header("content-type").starts_with("text/html"));
    }
    let r = app
        .send(Request::head("/calendar").body(Body::empty()).unwrap())
        .await;
    assert_eq!(r.status, 200);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn the_shell_policy_names_its_inline_script_by_hash() {
    let (app, dir) = app_with_frontend().await;
    let csp = app
        .get("/")
        .await
        .header("content-security-policy")
        .to_string();
    let digest = base64::engine::general_purpose::STANDARD
        .encode(mecloud_server::crypto::sha256(BOOT.as_bytes()));
    let script_src = csp
        .split_once("script-src")
        .unwrap()
        .1
        .split_once(';')
        .unwrap()
        .0
        .to_string();
    assert_eq!(script_src.trim(), format!("'self' 'sha256-{digest}'"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn server_paths_are_never_answered_with_the_shell() {
    let (app, dir) = app_with_frontend().await;
    // A GET to a POST-only route once fell through to the catch-all and got
    // index.html with a 200.
    let r = app.get("/auth/logout").await;
    assert!(r.status == 405 || r.status == 404);
    assert!(!r.text().contains("__sveltekit_boot"));
    for path in ["/auth/nope", "/api/nope", "/api/jmap/nope"] {
        let r = app.get(path).await;
        assert_eq!(r.status, 404, "{path}");
        assert_eq!(r.json()["detail"], "Not found");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn traversal_stays_inside_the_build() {
    let (app, dir) = app_with_frontend().await;
    for path in [
        "/../../../../etc/passwd",
        "/%2e%2e/%2e%2e/etc/passwd",
        "/_app/../../etc/passwd",
        "//etc/passwd",
    ] {
        let r = app.get(path).await;
        assert!(!r.text().contains("root:"), "{path}");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn other_methods_on_app_routes_are_405() {
    let (app, dir) = app_with_frontend().await;
    let r = app.post("/calendar", Body::empty()).await;
    assert_eq!(r.status, 405);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn without_a_build_unknown_paths_are_404() {
    let app = TestApp::new().await;
    let r = app.get("/calendar").await;
    assert_eq!(r.status, 404);
    assert_eq!(r.json()["detail"], "Not Found");
}
