//! Sign-in and token refresh: once per refresh token, and never a sign-out
//! for a hiccup.

mod common;

use std::sync::atomic::Ordering;

use common::*;
use mecloud_server::crypto::now_f64;
use serde_json::json;

fn query_of(location: &str) -> std::collections::HashMap<String, String> {
    let query = location.split_once('?').map(|(_, q)| q).unwrap_or("");
    form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

#[tokio::test]
async fn the_whole_login() {
    let app = TestApp::new().await;
    let r = app.get("/auth/login").await;
    assert_eq!(r.status, 307);
    let location = r.header("location").to_string();
    assert!(location.starts_with(&format!("{}/authorize?", app.fake.base())));
    let q = query_of(&location);
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["client_id"], "test-client");
    assert_eq!(q["redirect_uri"], "http://localhost:8000/auth/callback");
    assert_eq!(q["code_challenge_method"], "S256");
    assert!(q["scope"].contains("urn:ietf:params:oauth:scope:files"));
    assert!(
        location.contains("scope=openid+offline_access+"),
        "spaces as +, as urlencode did"
    );
    let session = app.session();
    assert_eq!(session["oauth_state"], q["state"].as_str());

    let r = app
        .get(&format!(
            "/auth/callback?code=the-code&state={}",
            q["state"]
        ))
        .await;
    assert_eq!(r.status, 307, "{}", r.text());
    assert_eq!(r.header("location"), "/");
    // The token endpoint got the code, the secret and the PKCE verifier.
    let form = app.fake.token_requests.lock().unwrap()[0].clone();
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["code"], "the-code");
    assert_eq!(form["client_secret"], "test-secret");
    let verifier = &form["code_verifier"];
    let challenge = mecloud_server::crypto::B64URL_NOPAD
        .encode(mecloud_server::crypto::sha256(verifier.as_bytes()));
    assert_eq!(q["code_challenge"], challenge);

    // The session is rotated: tokens in, the login's state out.
    let session = app.session();
    assert_eq!(session["access_token"], "new-token");
    assert_eq!(session["refresh_token"], "rt-2");
    assert!(session["token_expires_at"].as_f64().unwrap() > now_f64() + 3500.0);
    assert!(!session.contains_key("oauth_state") && !session.contains_key("code_verifier"));
    assert_eq!(app.get("/auth/me").await.json()["authenticated"], true);
}

use base64::Engine as _;

#[tokio::test]
async fn a_bad_state_starts_the_login_over() {
    let app = TestApp::new().await;
    app.get("/auth/login").await;
    let r = app.get("/auth/callback?code=abc123&state=bad-state").await;
    assert_eq!(r.status, 307);
    assert_eq!(r.header("location"), "/auth/login");
    assert_eq!(app.fake.token_calls.load(Ordering::SeqCst), 0);
    // Without a login in progress at all, the same.
    let fresh = TestApp::new().await;
    assert_eq!(
        fresh
            .get("/auth/callback?code=a&state=b")
            .await
            .header("location"),
        "/auth/login"
    );
    // And the parameters are required.
    assert_eq!(fresh.get("/auth/callback?code=a").await.status, 422);
}

#[tokio::test]
async fn a_failed_exchange_is_502() {
    let app = TestApp::new().await;
    let location = app.get("/auth/login").await.header("location").to_string();
    app.fake
        .answer_token(400, json!({"error": "invalid_grant"}));
    let r = app
        .get(&format!(
            "/auth/callback?code=c&state={}",
            query_of(&location)["state"]
        ))
        .await;
    assert_eq!(r.status, 502);
    assert_eq!(r.json()["detail"], "Authentication failed");
}

#[tokio::test]
async fn the_destination_after_login_is_kept_but_must_be_local() {
    let app = TestApp::new().await;
    // Asking to pair while signed out remembers where to come back to.
    let pairing = "/auth/device?redirect=http%3A%2F%2F127.0.0.1%3A41234%2Fpaired&name=Laptop";
    let r = app.get(pairing).await;
    assert_eq!(r.status, 302);
    assert_eq!(r.header("location"), "/auth/login");
    assert_eq!(app.session()["after_login"], pairing);
    let location = app.get("/auth/login").await.header("location").to_string();
    let r = app
        .get(&format!(
            "/auth/callback?code=c&state={}",
            query_of(&location)["state"]
        ))
        .await;
    assert_eq!(r.header("location"), pairing);
    assert!(!app.session().contains_key("after_login"));
}

#[tokio::test]
async fn discovery_refuses_endpoints_off_the_mail_server() {
    let fake = start_fake().await;
    // The fake's discovery names its own base; claim the mail server lives
    // elsewhere and the endpoints no longer match it.
    let app = TestApp::from_settings(
        settings(&fake.base().replace("127.0.0.1", "localhost"), &[]),
        fake,
    );
    let r = app.get("/auth/login").await;
    assert_eq!(r.status, 502);
}

fn expiring(app: &TestApp) {
    app.sign_in(json!({"access_token": "old", "refresh_token": "rt-1", "token_expires_at": now_f64() + 60.0}));
}

#[tokio::test]
async fn concurrent_requests_share_one_refresh() {
    let app = TestApp::new().await;
    expiring(&app);
    app.fake.token_delay_ms.store(100, Ordering::SeqCst);
    let calls: Vec<_> = (0..10)
        .map(|_| app.post_json("/api/jmap", echo_request()))
        .collect();
    for r in futures_util::future::join_all(calls).await {
        assert_eq!(r.status, 200);
    }
    assert_eq!(app.fake.token_calls.load(Ordering::SeqCst), 1);
    let auths: std::collections::HashSet<String> = app
        .fake
        .jmap_requests
        .lock()
        .unwrap()
        .iter()
        .map(|(a, _)| a.clone())
        .collect();
    assert_eq!(auths, ["Bearer new-token".to_string()].into());
}

#[tokio::test]
async fn a_late_request_with_the_old_cookie_reuses_the_result() {
    let app = TestApp::new().await;
    expiring(&app);
    let old_cookie = app.cookie.lock().unwrap().clone();
    assert_eq!(app.post_json("/api/jmap", echo_request()).await.status, 200);
    // A rotating server refuses a spent refresh token.
    app.fake
        .answer_token(400, json!({"error": "invalid_grant"}));
    *app.cookie.lock().unwrap() = old_cookie;
    let r = app.post_json("/api/jmap", echo_request()).await;
    assert_eq!(r.status, 200);
    assert_eq!(app.fake.token_calls.load(Ordering::SeqCst), 1);
    assert_eq!(app.session()["access_token"], "new-token");
}

#[tokio::test]
async fn a_rejected_refresh_token_signs_out() {
    for status in [400, 401] {
        let app = TestApp::new().await;
        expiring(&app);
        app.fake
            .answer_token(status, json!({"error": "invalid_grant"}));
        let r = app.post_json("/api/jmap", echo_request()).await;
        assert_eq!(r.status, 401);
        assert!(r.header("set-cookie").contains("expires=Thu, 01 Jan 1970"));
        assert!(app.cookie.lock().unwrap().is_none());
    }
}

#[tokio::test]
async fn a_token_response_without_a_token_signs_out() {
    let app = TestApp::new().await;
    expiring(&app);
    app.fake.answer_token(200, json!({"token_type": "bearer"}));
    assert_eq!(app.post_json("/api/jmap", echo_request()).await.status, 401);
}

#[tokio::test]
async fn a_transient_failure_keeps_the_session_and_the_live_token() {
    let app = TestApp::new().await;
    expiring(&app);
    app.fake.answer_token(502, json!({}));
    let r = app.post_json("/api/jmap", echo_request()).await;
    assert_eq!(r.status, 200);
    assert_eq!(app.fake.last_jmap().0, "Bearer old");
    assert_eq!(app.session()["refresh_token"], "rt-1");
}

#[tokio::test]
async fn a_transient_failure_after_expiry_is_503_not_a_sign_out() {
    let app = TestApp::new().await;
    app.sign_in(json!({"access_token": "old", "refresh_token": "rt-1", "token_expires_at": now_f64() - 10.0}));
    app.fake.answer_token(502, json!({}));
    let r = app.post_json("/api/jmap", echo_request()).await;
    assert_eq!(r.status, 503);
    assert_eq!(r.header("retry-after"), "10");
    assert!(r.set_cookies().is_empty(), "the session is untouched");
    assert_eq!(app.session()["refresh_token"], "rt-1");
}

#[tokio::test]
async fn a_successful_refresh_updates_the_session() {
    let app = TestApp::new().await;
    expiring(&app);
    assert_eq!(app.post_json("/api/jmap", echo_request()).await.status, 200);
    let session = app.session();
    assert_eq!(session["access_token"], "new-token");
    assert_eq!(session["refresh_token"], "rt-2");
    assert!(session["token_expires_at"].as_f64().unwrap() > now_f64() + 3000.0);
    let form = app.fake.token_requests.lock().unwrap()[0].clone();
    assert_eq!(form["grant_type"], "refresh_token");
    assert_eq!(form["refresh_token"], "rt-1");
}

#[tokio::test]
async fn no_refresh_token_means_sign_in_again() {
    let app = TestApp::new().await;
    app.sign_in(json!({"access_token": "old", "token_expires_at": now_f64() + 60.0}));
    assert_eq!(app.post_json("/api/jmap", echo_request()).await.status, 401);
}

#[tokio::test]
async fn a_cookie_past_half_its_life_is_reissued() {
    let app = TestApp::with(&[("SESSION_MAX_AGE", "4")]).await;
    app.sign_in_with_token("tok");
    // Fresh: nothing to re-issue.
    assert!(app.get("/auth/me").await.set_cookies().is_empty());
    tokio::time::sleep(std::time::Duration::from_millis(3100)).await;
    let r = app.get("/auth/me").await;
    assert_eq!(r.json()["authenticated"], true);
    assert_eq!(r.set_cookies().len(), 1, "re-issued past half its life");
}
