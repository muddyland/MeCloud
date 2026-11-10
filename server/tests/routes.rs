//! Blobs, the event stream, the desktop downloads, and device pairing —
//! through the routes.

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use common::*;
use http_body_util::BodyExt;
use mecloud_server::{devices, downloads, pages};
use serde_json::json;
use tower::ServiceExt;

fn put_blob(app: &TestApp, id: &str, content_type: &str, bytes: &[u8]) {
    app.fake
        .blobs
        .lock()
        .unwrap()
        .insert(id.into(), (content_type.into(), bytes.to_vec()));
}

// ── Blobs ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn an_upload_streams_through_and_returns_the_blob_id() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    let r = app
        .send(
            Request::post("/api/files/blob")
                .header("content-type", "image/png; x=y")
                .body(Body::from("fakepng"))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(
        r.json(),
        json!({"blobId": "G7", "type": "image/png", "size": 7})
    );
    let (account, content_type, bytes) = app.fake.uploads.lock().unwrap()[0].clone();
    assert_eq!(
        (account.as_str(), content_type.as_str(), bytes.as_slice()),
        ("a1", "image/png", b"fakepng".as_slice())
    );
}

#[tokio::test]
async fn hostile_and_missing_blob_ids() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    let r = app.get("/api/files/blob/a%2f..%2f").await;
    assert_eq!(r.status, 400);
    assert_eq!(r.json()["detail"], "Malformed blob id");
    let r = app.get("/api/files/blob/nothere").await;
    assert_eq!(r.status, 404);
    assert_eq!(r.json()["detail"], "Blob not found");
    assert_eq!(
        app.get("/api/files/blob/x?inline=perhaps").await.status,
        422
    );
}

#[tokio::test]
async fn html_is_defanged_even_when_inline_is_asked_for() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    put_blob(&app, "B1", "text/html", b"<script>alert(1)</script>");
    let r = app
        .get("/api/files/blob/B1?name=evil.html&type=text/html&inline=true")
        .await;
    assert_eq!(r.status, 200);
    assert!(r
        .header("content-type")
        .starts_with("application/octet-stream"));
    assert!(r.header("content-disposition").starts_with("attachment;"));
    assert_eq!(r.header("x-content-type-options"), "nosniff");
    assert!(r.header("content-security-policy").contains("sandbox"));
    assert!(r.header("cache-control").contains("no-store"));
    assert_eq!(r.body.as_ref(), b"<script>alert(1)</script>");
}

#[tokio::test]
async fn images_and_pdfs_preview_inline_unsandboxed() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    put_blob(&app, "P1", "image/png", b"\x89PNG");
    put_blob(&app, "D1", "application/pdf", b"%PDF-1.7");
    for (id, name, kind) in [
        ("P1", "cat.png", "image/png"),
        ("D1", "doc.pdf", "application/pdf"),
    ] {
        let r = app
            .get(&format!(
                "/api/files/blob/{id}?name={name}&type={kind}&inline=1"
            ))
            .await;
        assert_eq!(r.status, 200);
        assert!(r.header("content-type").starts_with(kind));
        assert!(r.header("content-disposition").starts_with("inline;"));
        let policy = r.header("content-security-policy");
        assert!(
            !policy.contains("sandbox")
                && policy.contains("default-src 'none'")
                && !policy.contains("script-src")
        );
        assert_eq!(r.header("content-length"), r.body.len().to_string());
    }
    // The upstream's type is used when the client names none.
    let r = app.get("/api/files/blob/P1?inline=true").await;
    assert!(r.header("content-type").starts_with("image/png"));
}

#[tokio::test]
async fn non_ascii_names_make_valid_headers() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    put_blob(&app, "T1", "text/plain", b"hello");
    for name in ["report.txt", "Résumé.txt", "文件.txt", "Отчёт.txt"] {
        let encoded = mecloud_server::crypto::quote(name);
        let r = app
            .get(&format!(
                "/api/files/blob/T1?name={encoded}&type=text/plain&inline=true"
            ))
            .await;
        assert_eq!(r.status, 200);
        assert_eq!(r.text(), "hello");
        assert!(r
            .header("content-disposition")
            .contains(&format!("filename*=UTF-8''{encoded}")));
    }
}

// ── Event stream ────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_event_stream_is_proxied_and_released() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    let cookie = app.cookie.lock().unwrap().clone().unwrap();
    let request = Request::get("/api/jmap/events")
        .header("host", "localhost")
        .header("cookie", cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    let mut body = response.into_body();
    let first = tokio::time::timeout(Duration::from_secs(5), body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(first.data_ref().unwrap()).starts_with("event: state"));
    let seen = app.fake.event_auth.lock().unwrap()[0].clone();
    assert!(
        seen.starts_with("Bearer tok ") && seen.contains("types=*&closeafter=no&ping=30"),
        "{seen}"
    );
    assert_eq!(app.fake.events_open.load(Ordering::SeqCst), 1);
    assert_eq!(app.state.open_streams.load(Ordering::SeqCst), 1);

    // The browser goes away: the upstream connection and the slot go too.
    drop(body);
    for _ in 0..50 {
        if app.fake.events_open.load(Ordering::SeqCst) == 0
            && app.state.open_streams.load(Ordering::SeqCst) == 0
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the upstream stream was not released");
}

#[tokio::test]
async fn event_streams_are_capped() {
    let app = TestApp::with(&[("MAX_EVENT_STREAMS", "1")]).await;
    app.sign_in_with_token("tok");
    let cookie = app.cookie.lock().unwrap().clone().unwrap();
    let open = || {
        Request::get("/api/jmap/events")
            .header("host", "localhost")
            .header("cookie", cookie.clone())
            .body(Body::empty())
            .unwrap()
    };
    let mut first = app
        .router
        .clone()
        .oneshot(open())
        .await
        .unwrap()
        .into_body();
    first.frame().await.unwrap().unwrap();
    // The second is refused: it ends at once rather than queueing.
    let second = app.router.clone().oneshot(open()).await.unwrap();
    let rest = tokio::time::timeout(Duration::from_secs(5), second.into_body().collect())
        .await
        .unwrap()
        .unwrap();
    assert!(rest.to_bytes().is_empty());
}

// ── Desktop downloads ───────────────────────────────────────────────────────

fn download_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mecloud-downloads-{}",
        mecloud_server::crypto::sha256_hex(&mecloud_server::crypto::random_bytes::<8>())
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("mecloud-desktop-linux-x86_64.tar.gz"),
        b"tarball-bytes",
    )
    .unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        br#"{"version": "0.9.0", "artifacts": {"mecloud-desktop-linux-x86_64.tar.gz": {"sha256": "abc123"}}}"#,
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn releases_tokens_and_downloads() {
    let dir = download_dir();
    let app = TestApp::with(&[("DESKTOP_DOWNLOAD_DIR", dir.to_str().unwrap())]).await;
    app.sign_in_with_token("tok");
    let r = app.get("/api/desktop/releases").await;
    assert_eq!(r.json()["version"], "0.9.0");
    let artifacts = r.json()["artifacts"].clone();
    assert_eq!(artifacts.as_array().unwrap().len(), 1);
    assert_eq!(artifacts[0]["key"], "linux-x86_64");
    assert_eq!(artifacts[0]["size"], 13);
    assert_eq!(artifacts[0]["sha256"], "abc123");

    assert_eq!(
        app.post("/api/desktop/token?artifact=source", Body::empty())
            .await
            .status,
        404,
        "not shipped"
    );
    assert_eq!(
        app.post("/api/desktop/token", Body::empty()).await.status,
        422
    );
    let r = app
        .post("/api/desktop/token?artifact=linux-x86_64", Body::empty())
        .await;
    let minted = r.json();
    assert_eq!(minted["url"], "/api/desktop/download/linux-x86_64");
    assert_eq!(minted["expires_in"], 300);
    assert_eq!(minted["filename"], "mecloud-desktop-linux-x86_64.tar.gz");

    // The link works without the session.
    *app.cookie.lock().unwrap() = None;
    let url = format!(
        "{}?token={}",
        minted["url"].as_str().unwrap(),
        minted["token"].as_str().unwrap()
    );
    let r = app.get(&url).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body.as_ref(), b"tarball-bytes");
    assert_eq!(r.header("content-type"), "application/gzip");
    assert_eq!(
        r.header("content-disposition"),
        "attachment; filename=\"mecloud-desktop-linux-x86_64.tar.gz\""
    );
    assert!(r.header("cache-control").contains("no-store"));

    let r = app
        .get("/api/desktop/download/linux-x86_64?token=forged.token")
        .await;
    assert_eq!(r.status, 403);
    let source_token = downloads::mint(
        &app.state.download_key,
        "source",
        mecloud_server::crypto::now_secs(),
    );
    assert_eq!(
        app.get(&format!(
            "/api/desktop/download/linux-x86_64?token={source_token}"
        ))
        .await
        .status,
        403
    );
    assert_eq!(
        app.get("/api/desktop/download/..%2F..%2Fetc%2Fpasswd?token=x")
            .await
            .status,
        404
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn a_build_without_the_desktop_offers_nothing() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    assert_eq!(
        app.get("/api/desktop/releases").await.json(),
        json!({"version": null, "artifacts": []})
    );
}

// ── Device pairing ──────────────────────────────────────────────────────────

#[tokio::test]
async fn pairing_refuses_bad_addresses_and_states() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    let r = app
        .get("/auth/device?redirect=http%3A%2F%2Flocalhost%3A41234%2Fpaired")
        .await;
    assert_eq!(r.status, 400);
    let r = app
        .get("/auth/device?redirect=http%3A%2F%2F127.0.0.1%3A41234%2Fpaired&state=short")
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.json()["detail"], "Invalid pairing request.");
}

#[tokio::test]
async fn the_consent_page_names_the_device_and_the_account() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    let r = app.get("/auth/device?redirect=http%3A%2F%2F127.0.0.1%3A41234%2Fpaired&name=%3Cb%3ELaptop&state=ssssssssssssssssssssssss").await;
    assert_eq!(r.status, 200);
    let page = r.text();
    assert!(page.contains("Connect &lt;b&gt;Laptop?"));
    assert!(page.contains(&format!("Signed in as {USERNAME}")));
    assert!(page.contains("action=\"/auth/device/approve\""));
    assert_eq!(
        r.header("cache-control"),
        "no-store, no-cache, must-revalidate, private"
    );
    let sealed = page
        .split_once("name=\"request_token\" value=\"")
        .unwrap()
        .1
        .split_once('"')
        .unwrap()
        .0;
    let opened = devices::open_pairing(&app.state.pairing_key, sealed).unwrap();
    assert_eq!(
        opened,
        (
            "http://127.0.0.1:41234/paired".into(),
            "<b>Laptop".into(),
            "ssssssssssssssssssssssss".into()
        )
    );
}

#[tokio::test]
async fn approving_pairs_a_device_that_then_works_on_its_own() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    let state = "s".repeat(32);
    let sealed = devices::seal_pairing(
        &app.state.pairing_key,
        "http://127.0.0.1:41234/paired",
        "Laptop",
        &state,
    );
    app.fake.answer_jmap(200, json!({"methodResponses": [["x:AppPassword/set", {"created": {"new": {"id": "p1", "secret": "s3cret"}}}, "0"]]}));
    let body = format!("request_token={}", mecloud_server::crypto::quote(&sealed));
    let r = app
        .send(
            Request::post("/auth/device/approve")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let csp = r.header("content-security-policy");
    assert!(csp.contains("form-action http://127.0.0.1:41234;"), "{csp}");
    assert_eq!(r.header("referrer-policy"), "no-referrer");
    let page = r.text();
    assert!(page.contains(&format!("name=\"state\" value=\"{state}\"")));
    assert!(page.contains("method=\"post\" action=\"http://127.0.0.1:41234/paired\""));

    // What was asked of the mail server.
    let (auth, sent) = app.fake.last_jmap();
    assert_eq!(auth, "Bearer tok");
    let call = &sent["methodCalls"][0];
    assert_eq!(call[0], "x:AppPassword/set");
    assert_eq!(call[1]["accountId"], "a1");
    assert_eq!(
        call[1]["create"]["new"],
        json!({"description": "Laptop", "expiresAt": null})
    );
    assert_eq!(
        sent["using"].as_array().unwrap().len(),
        3,
        "every advertised capability"
    );

    // The token is a device credential, used as Basic upstream.
    let token = page
        .split_once("name=\"token\" value=\"")
        .unwrap()
        .1
        .split_once('"')
        .unwrap()
        .0;
    let device =
        devices::read(&app.state.device_key, token, devices::DEVICE_TOKEN_MAX_AGE).unwrap();
    assert_eq!(
        (
            device.username.as_str(),
            device.secret.as_str(),
            device.password_id.as_str()
        ),
        (USERNAME, "s3cret", "p1")
    );
    *app.cookie.lock().unwrap() = None;
    let r = app
        .send(
            Request::post("/api/jmap")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(echo_request().to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(app.fake.last_jmap().0, device.basic_credential());
}

#[tokio::test]
async fn a_legacy_client_gets_the_get_handoff() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    let sealed = devices::seal_pairing(
        &app.state.pairing_key,
        "http://127.0.0.1:41234/paired",
        "Laptop",
        "",
    );
    app.fake.answer_jmap(200, json!({"methodResponses": [["x:AppPassword/set", {"created": {"new": {"id": "p1", "secret": "pw"}}}, "0"]]}));
    let r = app
        .post("/auth/device/approve", format!("request_token={sealed}"))
        .await;
    let page = r.text();
    assert!(page
        .contains("http-equiv=\"refresh\" content=\"0;url=http://127.0.0.1:41234/paired?token="));
}

#[tokio::test]
async fn approval_needs_a_session_and_a_fresh_request() {
    let app = TestApp::new().await;
    assert_eq!(
        app.post("/auth/device/approve", "request_token=x")
            .await
            .status,
        401
    );
    app.sign_in_with_token("tok");
    let r = app.post("/auth/device/approve", "request_token=x").await;
    assert_eq!(r.status, 400);
    assert_eq!(
        r.json()["detail"],
        "This pairing request has expired. Try again."
    );
}

#[tokio::test]
async fn a_bad_device_token_is_refused_and_never_falls_back_to_the_cookie() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    let r = app
        .send(
            Request::get("/api/jmap/session")
                .header("authorization", "Bearer garbage")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json()["detail"], "This device is no longer paired");
}

// ── App passwords on Stalwart 1.0.3, where OAuth tokens are scoped ──────────

fn refusal(description: &str) -> serde_json::Value {
    json!({"methodResponses": [["x:AppPassword/set", {"notCreated": {"new": {"type": "forbidden", "description": description}}}, "0"]]})
}

fn created() -> serde_json::Value {
    json!({"methodResponses": [["x:AppPassword/set", {"created": {"new": {"id": "p1", "secret": "s3cret"}}}, "0"]]})
}

async fn approve(app: &TestApp) -> TestResponse {
    let sealed = devices::seal_pairing(
        &app.state.pairing_key,
        "http://127.0.0.1:41234/paired",
        "x",
        &"s".repeat(32),
    );
    app.post("/auth/device/approve", format!("request_token={sealed}"))
        .await
}

fn sent_permissions(app: &TestApp) -> Vec<Option<serde_json::Value>> {
    app.fake
        .jmap_requests
        .lock()
        .unwrap()
        .iter()
        .map(|(_, body)| {
            body["methodCalls"][0][1]["create"]["new"]
                .get("permissions")
                .cloned()
        })
        .collect()
}

#[tokio::test]
async fn a_scoped_token_lists_the_interoperable_permissions_minus_refusals() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    app.fake.answer_jmap(
        200,
        refusal(
            "You are not authorized to grant permissions: interactAi, sysAccountSettingsUpdate",
        ),
    );
    app.fake.answer_jmap(
        200,
        refusal("You are not authorized to grant permissions: davCalAcl."),
    );
    app.fake.answer_jmap(200, created());
    assert_eq!(approve(&app).await.status, 200);
    let sent = sent_permissions(&app);
    assert_eq!(sent.len(), 3);
    assert!(
        sent[0].is_none(),
        "the account's permissions are asked for first"
    );
    let second = sent[1].clone().unwrap();
    assert_eq!(second["@type"], "Replace");
    let names: Vec<&str> = second["permissions"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        names,
        mecloud_server::permissions::INTEROPERABLE_PERMISSIONS
    );
    let third = sent[2].clone().unwrap();
    assert!(third["permissions"].get("davCalAcl").is_none());
    assert_eq!(third["permissions"]["jmapFileNodeGet"], true);
}

#[tokio::test]
async fn an_unexplained_refusal_is_reported() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    app.fake.answer_jmap(
        200,
        refusal("You are not authorized to grant permissions: interactAi"),
    );
    app.fake.answer_jmap(200, json!({"methodResponses": [["x:AppPassword/set", {"notCreated": {"new": {"type": "forbidden"}}}, "0"]]}));
    let r = approve(&app).await;
    assert_eq!(r.status, 502);
    assert_eq!(
        r.json()["detail"],
        "The mail server would not create a password for this device"
    );
    assert_eq!(sent_permissions(&app).len(), 2);
}

// ── Pages ───────────────────────────────────────────────────────────────────

async fn body_text(response: axum::response::Response) -> (axum::http::HeaderMap, String) {
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (headers, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn the_handoff_page_is_a_navigation_and_escapes_its_target() {
    let target = "http://127.0.0.1:41234/paired?token=abc";
    let (_, body) = body_text(pages::handoff_page(target)).await;
    assert!(
        body.contains("http-equiv=\"refresh\"")
            && body.contains(target)
            && body.contains(&format!("href=\"{target}\""))
    );
    let (_, body) = body_text(pages::handoff_page(
        "http://127.0.0.1:1/p?token=a\"><script>x</script>",
    ))
    .await;
    assert!(!body.contains("<script>") && body.contains("&lt;script&gt;"));
}

#[tokio::test]
async fn the_form_handoff_pins_its_script_and_escapes_its_values() {
    let (headers, body) = body_text(pages::handoff_form(
        "http://127.0.0.1:41234/paired",
        "tok/+=",
        &"s".repeat(32),
    ))
    .await;
    let csp = headers["content-security-policy"].to_str().unwrap();
    assert!(body.contains("name=\"token\" value=\"tok/+=\""));
    assert!(
        !body.split_once("<form").unwrap().0.contains("token="),
        "never in a URL"
    );
    assert!(csp.contains("form-action http://127.0.0.1:41234;"));
    let script = body
        .split_once("<script>")
        .unwrap()
        .1
        .split_once("</script>")
        .unwrap()
        .0;
    let digest = base64::engine::general_purpose::STANDARD
        .encode(mecloud_server::crypto::sha256(script.as_bytes()));
    assert!(csp.contains(&format!("'sha256-{digest}'")));
    let script_src = csp
        .split_once("script-src")
        .unwrap()
        .1
        .split_once(';')
        .unwrap()
        .0;
    assert!(!script_src.contains("unsafe-inline"));
    let (_, body) = body_text(pages::handoff_form(
        "http://127.0.0.1:1/p",
        "\"><script>x</script>",
        &"s".repeat(32),
    ))
    .await;
    assert!(!body.contains("<script>x"));
}

use base64::Engine as _;
