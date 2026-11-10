//! The API surface: health, config, the JMAP proxy, Sieve, headers, limits
//! and error mapping.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::*;
use serde_json::json;

#[tokio::test]
async fn health_and_config() {
    let app = TestApp::new().await;
    let r = app.get("/health").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json(), json!({"status": "ok"}));
    let r = app.get("/api/config").await;
    assert_eq!(
        r.json(),
        json!({"appName": "Test Mail", "stalwartUrl": app.fake.base()})
    );
}

#[tokio::test]
async fn authenticated_endpoints_reject_unauthenticated() {
    let app = TestApp::new().await;
    for (method, path) in [
        ("GET", "/api/jmap/session"),
        ("POST", "/api/jmap"),
        ("GET", "/api/jmap/events"),
        ("GET", "/api/sieve"),
        ("PUT", "/api/sieve"),
        ("GET", "/api/desktop/releases"),
        ("POST", "/api/desktop/token?artifact=source"),
        ("POST", "/api/files/blob"),
        ("GET", "/api/files/blob/abc123"),
    ] {
        let body = if method == "GET" {
            Body::empty()
        } else {
            Body::from("{}")
        };
        let r = app
            .send(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(body)
                    .unwrap(),
            )
            .await;
        assert_eq!(r.status, 401, "{method} {path}");
        assert_eq!(r.json()["detail"], "Not authenticated");
    }
}

#[tokio::test]
async fn me_reports_the_session() {
    let app = TestApp::new().await;
    assert_eq!(
        app.get("/auth/me").await.json(),
        json!({"authenticated": false, "email": null})
    );
    app.sign_in_with_token("tok");
    assert_eq!(
        app.get("/auth/me").await.json(),
        json!({"authenticated": true, "email": null})
    );
}

#[tokio::test]
async fn logout_is_post_only_and_clears_the_cookie() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    let r = app.get("/auth/logout").await;
    assert_eq!(r.status, 405);
    assert_ne!(r.json(), json!({"ok": true}));
    let r = app.post("/auth/logout", Body::empty()).await;
    assert_eq!(r.json(), json!({"ok": true}));
    assert!(r.header("set-cookie").contains("expires=Thu, 01 Jan 1970"));
    assert_eq!(app.get("/auth/me").await.json()["authenticated"], false);
}

#[tokio::test]
async fn no_cookie_is_set_when_nothing_is_stored() {
    let app = TestApp::new().await;
    assert!(app.get("/health").await.set_cookies().is_empty());
    assert!(app.get("/api/config").await.set_cookies().is_empty());
}

#[tokio::test]
async fn a_tampered_cookie_is_an_empty_session() {
    let app = TestApp::new().await;
    *app.cookie.lock().unwrap() = Some("session=not-a-valid-fernet-token".into());
    assert_eq!(app.get("/auth/me").await.json()["authenticated"], false);
}

#[tokio::test]
async fn a_cookie_from_another_secret_is_rejected() {
    let first = TestApp::new().await;
    first.sign_in_with_token("tok");
    let cookie = first.cookie.lock().unwrap().clone();
    let other = TestApp::with(&[("SESSION_SECRET", "a-completely-different-secret-value")]).await;
    *other.cookie.lock().unwrap() = cookie;
    assert_eq!(other.get("/auth/me").await.json()["authenticated"], false);
}

// ── JMAP proxy ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_proxy_forwards_a_valid_request_with_the_session_token() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok-1");
    let r = app
        .post_json(
            "/api/jmap",
            json!({
                "using": ["urn:ietf:params:jmap:core"],
                "methodCalls": [["Core/echo", {"hello": true}, "c1"]],
                "createdIds": {"dropped": "x"},
            }),
        )
        .await;
    assert_eq!(r.status, 200);
    assert_eq!(
        r.json()["methodResponses"],
        json!([["Core/echo", {"hello": true}, "c1"]])
    );
    let (auth, sent) = app.fake.last_jmap();
    assert_eq!(auth, "Bearer tok-1");
    assert_eq!(
        sent,
        echo_request(),
        "only using and methodCalls go upstream"
    );
}

#[tokio::test]
async fn the_session_document_is_fetched_once_and_cached() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok-1");
    let calls: Vec<_> = (0..5)
        .map(|_| app.post_json("/api/jmap", echo_request()))
        .collect();
    for r in futures_util::future::join_all(calls).await {
        assert_eq!(r.status, 200);
    }
    assert_eq!(
        app.fake
            .session_fetches
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    // /api/jmap/session itself always asks afresh.
    let r = app.get("/api/jmap/session").await;
    assert_eq!(r.json()["username"], USERNAME);
    assert_eq!(
        app.fake
            .session_fetches
            .load(std::sync::atomic::Ordering::SeqCst),
        2
    );
}

#[tokio::test]
async fn malformed_envelopes_are_422() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    for body in [
        json!({"using": []}),
        json!({"using": ["urn:ietf:params:jmap:core"], "methodCalls": vec![json!(["Mailbox/get", {}, "q"]); 51]}),
        json!({"using": (0..21).map(|i| format!("urn:test:{i}")).collect::<Vec<_>>(), "methodCalls": [["Mailbox/get", {}, "q"]]}),
    ] {
        assert_eq!(app.post_json("/api/jmap", body).await.status, 422);
    }
    let r = app.post("/api/jmap", "{not json").await;
    assert_eq!(r.status, 422);
    assert!(r.json()["detail"].is_array());
    assert!(
        app.fake.jmap_requests.lock().unwrap().is_empty(),
        "nothing invalid went upstream"
    );
}

#[tokio::test]
async fn oversized_bodies_are_413_declared_or_not() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    let big = "x".repeat(1_048_576 + 1_000);
    let body = json!({"using": ["urn:ietf:params:jmap:core"], "methodCalls": [["Mailbox/get", {"filter": big}, "q"]]});
    let r = app.post_json("/api/jmap", body.clone()).await;
    assert_eq!(r.status, 413);
    assert_eq!(r.json()["detail"], "Request body too large");
    // Chunked, with no Content-Length to check up front.
    let chunks = futures_util::stream::iter(
        body.to_string()
            .into_bytes()
            .chunks(65_536)
            .map(|c| Ok::<_, std::io::Error>(c.to_vec()))
            .collect::<Vec<_>>(),
    );
    let r = app
        .send(
            Request::post("/api/jmap")
                .body(Body::from_stream(chunks))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 413);
    // The tight cap is the proxy's; uploads have their own.
    let r = app
        .send(
            Request::post("/api/files/blob")
                .header("content-length", "2000000")
                .body(Body::from(vec![b'x'; 2_000_000]))
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
}

#[tokio::test]
async fn upstream_failures_map_to_actionable_statuses() {
    let app = TestApp::new().await;
    // A revoked token: 401, and the session is cleared so the client signs in.
    app.sign_in_with_token("revoked-token");
    let r = app.post_json("/api/jmap", echo_request()).await;
    assert_eq!(r.status, 401);
    assert_eq!(r.json()["detail"], "Session expired, please log in again");
    assert!(r.header("set-cookie").contains("expires=Thu, 01 Jan 1970"));

    // A broken mail server: 502, without echoing its internals.
    app.sign_in_with_token("tok");
    app.fake.answer_jmap(500, json!({"internal": "trace"}));
    let r = app.post_json("/api/jmap", echo_request()).await;
    assert_eq!(r.status, 502);
    assert_eq!(r.json()["detail"], "Upstream service error");

    // Statuses the client can act on pass through.
    app.fake.answer_jmap(503, json!({}));
    assert_eq!(app.post_json("/api/jmap", echo_request()).await.status, 503);
}

#[tokio::test]
async fn an_unreachable_mail_server_is_504() {
    let fake = start_fake().await;
    // A port nothing listens on.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let app = TestApp::from_settings(settings(&url, &[]), fake);
    app.sign_in_with_token("tok");
    let r = app.post_json("/api/jmap", echo_request()).await;
    assert_eq!(r.status, 504);
    assert_eq!(r.json()["detail"], "Upstream service unavailable");
}

#[tokio::test]
async fn unknown_api_paths_are_a_json_404() {
    let app = TestApp::new().await;
    let r = app.get("/api/does-not-exist").await;
    assert_eq!(r.status, 404);
    assert_eq!(r.json()["detail"], "Not found");
    assert_eq!(
        app.post("/api/nope/deeper", Body::empty()).await.json()["detail"],
        "Not found"
    );
}

// ── Sieve ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn sieve_round_trip() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    // No scripts yet.
    app.fake.answer_jmap(
        200,
        json!({"methodResponses": [["SieveScript/get", {"list": []}, "s"]]}),
    );
    let r = app.get("/api/sieve").await;
    assert_eq!(
        r.json(),
        json!({"id": null, "name": "My Rules", "content": "", "isActive": false})
    );

    // Saving uploads the script, creates it, and activates it.
    app.fake.answer_jmap(
        200,
        json!({"methodResponses": [["SieveScript/set", {"created": {"new": {"id": "s9"}}}, "s"]]}),
    );
    app.fake.answer_jmap(
        200,
        json!({"methodResponses": [["SieveScript/set", {}, "sa"]]}),
    );
    let r = app
        .put_json(
            "/api/sieve",
            json!({"content": "require [];", "name": "Mine"}),
        )
        .await;
    assert_eq!(r.json(), json!({"id": "s9", "name": "Mine"}));
    let uploads = app.fake.uploads.lock().unwrap().clone();
    assert_eq!(uploads.last().unwrap().1, "application/sieve");
    assert_eq!(uploads.last().unwrap().2, b"require [];");
    let (_, activate) = app.fake.last_jmap();
    assert_eq!(
        activate["methodCalls"][0][1]["onSuccessActivateScript"],
        "s9"
    );

    // Reading it back fetches the active script's blob.
    let blob = format!("G{}", "require [];".len());
    app.fake.answer_jmap(
        200,
        json!({"methodResponses": [["SieveScript/get", {"list": [
        {"id": "s1", "name": "Old", "blobId": "none", "isActive": false},
        {"id": "s9", "name": "Mine", "blobId": blob, "isActive": true},
    ]}, "s"]]}),
    );
    let r = app.get("/api/sieve").await;
    assert_eq!(
        r.json(),
        json!({"id": "s9", "name": "Mine", "content": "require [];", "isActive": true})
    );

    // Updating an existing script names it.
    app.fake.answer_jmap(
        200,
        json!({"methodResponses": [["SieveScript/set", {"updated": {"s9": null}}, "s"]]}),
    );
    let r = app
        .put_json(
            "/api/sieve",
            json!({"id": "s9", "content": "keep;", "makeActive": false}),
        )
        .await;
    assert_eq!(r.json(), json!({"id": "s9", "name": "My Rules"}));
    let (_, update) = app.fake.last_jmap();
    assert!(update["methodCalls"][0][1]["update"]["s9"]["blobId"].is_string());
}

#[tokio::test]
async fn sieve_saves_are_validated() {
    let app = TestApp::new().await;
    app.sign_in_with_token("tok");
    assert_eq!(
        app.put_json("/api/sieve", json!({"content": "x".repeat(70_000)}))
            .await
            .status,
        422
    );
    assert_eq!(
        app.put_json(
            "/api/sieve",
            json!({"content": "x", "name": "n".repeat(300)})
        )
        .await
        .status,
        422
    );
}

// ── Headers ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn security_headers_are_present() {
    let app = TestApp::new().await;
    let r = app.get("/health").await;
    assert_eq!(r.header("x-content-type-options"), "nosniff");
    assert_eq!(r.header("x-frame-options"), "DENY");
    let csp = r.header("content-security-policy");
    for needed in [
        "object-src 'none'",
        "frame-src 'self'",
        "base-uri 'self'",
        "frame-ancestors 'none'",
    ] {
        assert!(csp.contains(needed), "{needed}");
    }
    let script_src = csp
        .split_once("script-src")
        .unwrap()
        .1
        .split_once(';')
        .unwrap()
        .0;
    assert!(!script_src.contains("'unsafe-inline'") && script_src.contains("'self'"));
    assert_eq!(
        r.header("referrer-policy"),
        "strict-origin-when-cross-origin"
    );
    assert_eq!(r.header("cross-origin-opener-policy"), "same-origin");
    // Only /api and /auth are no-store; development sends no HSTS.
    assert!(!r.header("cache-control").contains("no-store"));
    assert_eq!(r.header("strict-transport-security"), "");
    assert!(app
        .get("/api/config")
        .await
        .header("cache-control")
        .contains("no-store"));
    assert!(app
        .get("/auth/me")
        .await
        .header("cache-control")
        .contains("no-store"));
}

#[tokio::test]
async fn framing_can_be_allowed_for_one_origin() {
    let app = TestApp::with(&[("FRAME_ANCESTORS", "https://dash.example")]).await;
    let r = app.get("/health").await;
    assert_eq!(r.header("x-frame-options"), "");
    assert!(r
        .header("content-security-policy")
        .contains("frame-ancestors https://dash.example;"));
}

#[tokio::test]
async fn cors_allows_only_the_configured_origins() {
    let app = TestApp::new().await;
    let preflight = |origin: &'static str| {
        Request::builder()
            .method("OPTIONS")
            .uri("/api/jmap")
            .header("origin", origin)
            .header("access-control-request-method", "POST")
            .header("access-control-request-headers", "content-type")
            .body(Body::empty())
            .unwrap()
    };
    let r = app.send(preflight("http://localhost:5173")).await;
    assert_eq!(
        r.header("access-control-allow-origin"),
        "http://localhost:5173"
    );
    assert_eq!(r.header("access-control-allow-credentials"), "true");
    let r = app.send(preflight("https://evil.example")).await;
    assert_eq!(r.header("access-control-allow-origin"), "");
}

#[tokio::test]
async fn production_checks_the_host_and_sends_hsts() {
    let fake = start_fake().await;
    let s = settings(
        &fake.base(),
        &[
            ("ENVIRONMENT", "production"),
            ("APP_URL", "https://mail.example.com"),
            (
                "OAUTH_REDIRECT_URI",
                "https://mail.example.com/auth/callback",
            ),
            (
                "SESSION_SECRET",
                "0123456789abcdef0123456789abcdef0123456789abcdef",
            ),
        ],
    );
    let app = TestApp::from_settings(s, fake);
    for host in ["mail.example.com", "localhost:8000", "127.0.0.1"] {
        let r = app
            .send(
                Request::get("/health")
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(r.status, 200, "{host}");
        assert!(r
            .header("strict-transport-security")
            .contains("max-age=63072000"));
    }
    for host in ["evil.example", "172.17.0.2:8000", "mail.example.com.evil"] {
        let r = app
            .send(
                Request::get("/health")
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{host}");
        assert_eq!(r.text(), "Invalid host header");
    }
    // And the session cookie is __Host- and Secure.
    let r = app
        .send(
            Request::get("/auth/login")
                .header("host", "mail.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    let cookie = r.header("set-cookie");
    assert!(
        cookie.starts_with("__Host-session=")
            && cookie.contains("; secure")
            && cookie.contains("httponly")
    );
}

// ── Rate limits ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_rate_limited_response_says_when_to_retry() {
    let app = TestApp::new().await;
    let mut last = None;
    for _ in 0..25 {
        let r = app.get("/auth/login").await;
        if r.status == 429 {
            last = Some(r);
            break;
        }
        assert_eq!(r.status, 307);
    }
    let r = last.expect("the login limiter trips");
    assert!(r.header("retry-after").parse::<u64>().unwrap() > 0);
    assert!(r.json()["detail"]
        .as_str()
        .unwrap()
        .starts_with("Too many requests"));
}

#[tokio::test]
async fn limits_are_per_endpoint_not_per_url() {
    let app = TestApp::with(&[("RATE_LIMIT_DOWNLOAD", "3/minute")]).await;
    app.sign_in_with_token("tok");
    // Different blob ids, one bucket: the third distinct URL is still the
    // same endpoint.
    let mut statuses = Vec::new();
    for i in 0..5 {
        statuses.push(
            app.get(&format!("/api/files/blob/missing{i}"))
                .await
                .status
                .as_u16(),
        );
    }
    assert_eq!(statuses, [404, 404, 404, 429, 429]);
    // Another endpoint has its own allowance.
    assert_eq!(app.post_json("/api/jmap", echo_request()).await.status, 200);
}

#[tokio::test]
async fn forwarded_for_from_a_trusted_proxy_picks_the_bucket() {
    let app = TestApp::with(&[("RATE_LIMIT_DOWNLOAD", "1/minute")]).await;
    app.sign_in_with_token("tok");
    let from = |ip: &str| {
        Request::get("/api/files/blob/x")
            .header("x-forwarded-for", ip)
            .body(Body::empty())
            .unwrap()
    };
    // The test client is 127.0.0.1, the default trusted proxy.
    assert_eq!(app.send(from("10.0.0.1")).await.status, 404);
    assert_eq!(app.send(from("10.0.0.1")).await.status, 429);
    assert_eq!(app.send(from("10.0.0.2")).await.status, 404);
}

#[tokio::test]
async fn the_web_manifest() {
    let app = TestApp::new().await;
    let r = app.get("/manifest.webmanifest").await;
    let m = r.json();
    assert_eq!(m["name"], "Test Mail");
    assert_eq!(m["short_name"], "Test");
    assert_eq!(m["icons"].as_array().unwrap().len(), 4);
}
