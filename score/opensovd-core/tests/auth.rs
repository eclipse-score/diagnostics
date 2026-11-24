// *******************************************************************************
// Copyright (c) 2026 Contributors to the Eclipse Foundation
//
// See the NOTICE file(s) distributed with this work for additional
// information regarding copyright ownership.
//
// This program and the accompanying materials are made available under the
// terms of the Apache License Version 2.0 which is available at
// <https://www.apache.org/licenses/LICENSE-2.0>
//
// SPDX-License-Identifier: Apache-2.0
// *******************************************************************************

#[path = "../src/auth.rs"]
mod auth;

use opensovd_core::Topology;
use opensovd_server::Auth;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

struct TaskGuard<T>(JoinHandle<T>);

impl<T> Drop for TaskGuard<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Sends one request and returns the whole response as text.
async fn send(
    address: SocketAddr,
    method: &str,
    path: &str,
    authorization: Option<&str>,
    body: &str,
) -> String {
    let mut stream = TcpStream::connect(address)
        .await
        .expect("connect to gateway");
    let header = authorization.map_or(String::new(), |value| format!("Authorization: {value}\r\n"));
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{header}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("send request");
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut response))
        .await
        .expect("response timed out")
        .expect("read response");
    String::from_utf8(response).expect("HTTP response")
}

/// `GET /sovd/v1/functions`, a route that needs a token and that the upstream serves.
async fn request(address: SocketAddr, authorization: Option<&str>) -> String {
    send(address, "GET", "/sovd/v1/functions", authorization, "").await
}

/// `POST /sovd/v1/authorize` with `body`.
async fn authorize(address: SocketAddr, body: &str) -> String {
    send(address, "POST", "/sovd/v1/authorize", None, body).await
}

fn body_of(response: &str) -> &str {
    response.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// The token issued by an `authorize` response, after checking what the response says about it.
fn issued_token(response: &str) -> String {
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let lower = response.to_ascii_lowercase();
    assert!(lower.contains("cache-control: no-store"), "{response}");
    let body: serde_json::Value = serde_json::from_str(body_of(response)).expect("JSON body");
    assert_eq!(body["token_type"], "Bearer", "{body}");
    assert_eq!(body["expires_in"], 3600, "{body}");
    body["access_token"].as_str().expect("access_token").to_owned()
}

/// Decodes `token` as the CDA does with its `auth` feature: HS256, the key `secret` and
/// `Validation::default()`, which requires a numeric `exp` that has not passed.
fn decode_like_the_cda(token: &str) -> serde_json::Value {
    jsonwebtoken::decode::<serde_json::Value>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(b"secret"),
        &jsonwebtoken::Validation::default(),
    )
    .expect("the CDA's verification accepts the token")
    .claims
}

/// The requests a recording upstream received, as raw text.
type Recorded = Arc<Mutex<Vec<String>>>;

/// An upstream that records every request and answers `200 ok`. Returns its URL.
async fn recording_upstream() -> (TaskGuard<()>, String, Recorded) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("upstream listener");
    let url = format!(
        "http://{}/vehicle/v15",
        listener.local_addr().expect("upstream address")
    );
    let requests: Recorded = Arc::default();
    let recorded = requests.clone();
    let task = TaskGuard(tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream
                    .read(&mut buffer)
                    .await
                    .expect("read upstream request");
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..n]);
            }
            recorded
                .lock()
                .expect("requests")
                .push(String::from_utf8(request).expect("request"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .expect("upstream response");
        }
    }));
    (task, url, requests)
}

/// Starts the gateway on a free port, configured like `main` does: the placeholder authenticator
/// without `--auth-jwks`, Core's own authentication with it. Retries if another process takes the
/// port between reserving and binding it.
async fn start_gateway(upstream: &str, extra: &[&str]) -> (TaskGuard<()>, SocketAddr) {
    for _ in 0..5 {
        let reservation = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve port");
        let address = reservation.local_addr().expect("gateway address");
        let url = format!("http://{address}/sovd");
        let mut args = vec!["gateway", "--url", &url, "--upstream-url", upstream];
        args.extend_from_slice(extra);
        let options = opensovd_gateway::cli::try_parse_from(args).expect("options");
        let auth: Option<Auth> = auth::configured(&options);
        drop(reservation);
        let task = TaskGuard(tokio::spawn(async move {
            let _ = match auth {
                Some(auth) => {
                    opensovd_gateway::run_with_auth_and_shutdown(
                        options,
                        Topology::default(),
                        auth,
                        std::future::pending(),
                    )
                    .await
                },
                None => {
                    opensovd_gateway::run_with_shutdown(
                        options,
                        Topology::default(),
                        std::future::pending(),
                    )
                    .await
                },
            };
        }));
        for _ in 0..100 {
            if task.0.is_finished() {
                break;
            }
            if TcpStream::connect(address).await.is_ok() {
                return (task, address);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    panic!("gateway did not start");
}

fn forwarded_authorization(request: &str) -> Option<&str> {
    request.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("authorization").then(|| value.trim())
    })
}

#[tokio::test(flavor = "current_thread")]
async fn placeholder_requires_a_jwt_shaped_bearer_token_before_forwarding() {
    let (_upstream, upstream_url, requests) = recording_upstream().await;
    let (_gateway, address) = start_gateway(&upstream_url, &[]).await;

    for authorization in [
        None,
        Some("Basic dXNlcjpwYXNz"),
        Some("Bearer"),
        Some("Bearer not-a-jwt"),
        Some("Bearer two.segments"),
        Some("Bearer a..c"),
        Some("Bearer a.b.c.d"),
        Some("Bearer a.b.c+/="),
    ] {
        let rejected = request(address, authorization).await;
        assert!(rejected.starts_with("HTTP/1.1 401"), "{authorization:?}: {rejected}");
    }
    assert!(requests.lock().expect("requests").is_empty());

    // The signature is not checked: any JWT-shaped token passes.
    let token = format!("{}.{}.AAAA", TEST_JWT_HEADER, TEST_JWT_PAYLOAD);
    let accepted = request(address, Some(&format!("Bearer {token}"))).await;
    assert!(accepted.starts_with("HTTP/1.1 200"), "{accepted}");
    assert!(accepted.ends_with("ok"), "{accepted}");
    let requests = requests.lock().expect("requests");
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].starts_with("GET /vehicle/v15/functions "),
        "{}",
        requests[0]
    );
    assert_eq!(
        forwarded_authorization(&requests[0]),
        Some(format!("Bearer {token}").as_str())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn authorize_issues_a_token_that_the_gateway_accepts_and_forwards() {
    let (_upstream, upstream_url, requests) = recording_upstream().await;
    let (_gateway, address) = start_gateway(&upstream_url, &[]).await;

    let response = authorize(address, r#"{"client_id":"tester","client_secret":"anything"}"#).await;
    let token = issued_token(&response);

    // What the CDA reads from the forwarded token, also when it verifies tokens (its `auth`
    // feature): HS256 with the key `secret`, a string `sub` and a numeric `exp`.
    assert_eq!(token.split('.').count(), 3, "{token}");
    let claims = decode_like_the_cda(&token);
    assert!(
        jsonwebtoken::decode::<serde_json::Value>(
            &token,
            &jsonwebtoken::DecodingKey::from_secret(b"other"),
            &jsonwebtoken::Validation::default(),
        )
        .is_err(),
        "the signature is real"
    );
    assert_eq!(claims["sub"], "tester");
    let expires = claims["exp"].as_u64().expect("numeric exp");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_secs();
    assert!((now + 3500..=now + 3700).contains(&expires), "exp {expires}, now {now}");

    assert!(requests.lock().expect("requests").is_empty(), "authorize is not forwarded");
    let accepted = request(address, Some(&format!("Bearer {token}"))).await;
    assert!(accepted.starts_with("HTTP/1.1 200"), "{accepted}");
    assert!(accepted.ends_with("ok"), "{accepted}");
    let requests = requests.lock().expect("requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        forwarded_authorization(&requests[0]),
        Some(format!("Bearer {token}").as_str())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn authorize_rejects_requests_the_cda_rejects_and_empty_credentials() {
    let (_upstream, upstream_url, requests) = recording_upstream().await;
    let (_gateway, address) = start_gateway(&upstream_url, &[]).await;

    for body in [
        "",
        "not json",
        "[]",
        "null",
        "{}",
        r#"{"client_id":"tester"}"#,
        r#"{"client_secret":"anything"}"#,
        r#"{"client_id":1,"client_secret":"anything"}"#,
        r#"{"client_id":"tester","client_secret":null}"#,
        r#"{"client_id":"","client_secret":"anything"}"#,
        r#"{"client_id":"tester","client_secret":""}"#,
    ] {
        let rejected = authorize(address, body).await;
        assert!(rejected.starts_with("HTTP/1.1 400"), "{body:?}: {rejected}");
        let error: serde_json::Value = serde_json::from_str(body_of(&rejected)).expect("JSON error");
        assert!(error["error_code"].is_string() && error["message"].is_string(), "{error}");
        assert!(!rejected.contains("access_token"), "{rejected}");
    }
    // Unknown fields are ignored, and the credentials are not checked (placeholder).
    let extra = authorize(
        address,
        r#"{"client_id":"tester","client_secret":"wrong","grant_type":"client_credentials"}"#,
    )
    .await;
    issued_token(&extra);
    assert!(requests.lock().expect("requests").is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn only_version_info_and_authorize_are_open_to_anonymous_requests() {
    let (_upstream, upstream_url, requests) = recording_upstream().await;
    let (_gateway, address) = start_gateway(&upstream_url, &[]).await;

    // ISO 17978-3 §5.11: discovery reads version-info before the client has a token.
    let version_info = send(address, "GET", "/sovd/version-info", None, "").await;
    assert!(version_info.starts_with("HTTP/1.1 200"), "{version_info}");
    assert!(version_info.contains("sovd_info"), "{version_info}");
    let authorize = authorize(address, r#"{"client_id":"tester","client_secret":"x"}"#).await;
    assert!(authorize.starts_with("HTTP/1.1 200"), "{authorize}");

    // Everything else needs a token, whether the gateway serves it, would forward it, or has no
    // route for it. Nothing reaches the upstream.
    for (method, path) in [
        ("GET", "/sovd/v1/functions"),
        ("GET", "/sovd/v1/components"),
        ("GET", "/sovd/v1/components/ecu1/data"),
        ("GET", "/sovd/v1/x-acme-reports"),
        ("GET", "/sovd/v1/authorize"),
        ("POST", "/sovd/v1/token"),
        ("POST", "/sovd/version-info"),
        ("GET", "/sovd/v1/version-info"),
        ("GET", "/sovd/v1"),
        ("GET", "/sovd/v1/unknown"),
    ] {
        let rejected = send(address, method, path, None, "").await;
        assert!(rejected.starts_with("HTTP/1.1 401"), "{method} {path}: {rejected}");
    }
    assert!(requests.lock().expect("requests").is_empty());

    // With a token the same paths are served or forwarded as usual.
    let token = issued_token(&send(address, "POST", "/sovd/v1/authorize", None, r#"{"client_id":"t","client_secret":"x"}"#).await);
    let bearer = format!("Bearer {token}");
    let forwarded = send(address, "GET", "/sovd/v1/x-acme-reports", Some(&bearer), "").await;
    assert!(forwarded.starts_with("HTTP/1.1 200"), "{forwarded}");
    let requests = requests.lock().expect("requests");
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /vehicle/v15/x-acme-reports "), "{}", requests[0]);
}

#[tokio::test(flavor = "current_thread")]
async fn jwks_configuration_uses_core_signature_verification() {
    let dir = std::env::var_os("TEST_TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("gateway-jwt-forwarding-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("test directory");
    let _dir = TempDir(dir.clone());
    let jwks = dir.join("jwks.json");
    std::fs::write(&jwks, TEST_JWKS).expect("JWKS");
    let (_upstream, upstream_url, requests) = recording_upstream().await;
    let (_gateway, address) = start_gateway(
        &upstream_url,
        &[
            "--auth-jwks",
            jwks.to_str().expect("JWKS path"),
            "--auth-issuer",
            "test-issuer",
        ],
    )
    .await;

    // Right shape, wrong signature: the placeholder would accept these, Core must not.
    let wrong_signature = format!("{TEST_JWT_HEADER}.{TEST_JWT_PAYLOAD}.{}", "A".repeat(86));
    let unsigned = format!("{TEST_JWT_HEADER}.{TEST_JWT_PAYLOAD}.");
    for authorization in [
        None,
        Some(format!("Bearer {wrong_signature}")),
        Some(format!("Bearer {unsigned}")),
        Some("Bearer wrong".to_owned()),
    ] {
        let rejected = request(address, authorization.as_deref()).await;
        assert!(rejected.starts_with("HTTP/1.1 401"), "{authorization:?}: {rejected}");
    }
    // The external issuer issues the tokens: the gateway serves no `authorize` for anonymous callers.
    let anonymous = authorize(address, r#"{"client_id":"tester","client_secret":"x"}"#).await;
    assert!(anonymous.starts_with("HTTP/1.1 401"), "{anonymous}");
    assert!(requests.lock().expect("requests").is_empty());

    let accepted = request(address, Some(&format!("Bearer {TEST_JWT}"))).await;
    assert!(accepted.starts_with("HTTP/1.1 200"), "{accepted}");
    assert!(accepted.ends_with("ok"), "{accepted}");
    let requests = requests.lock().expect("requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        forwarded_authorization(&requests[0]),
        Some(format!("Bearer {TEST_JWT}").as_str())
    );
}

// Test-only Ed25519 public key and JWT, signed for test-issuer and expiring in 2100.
const TEST_JWKS: &str = r#"{"keys":[{"kty":"OKP","crv":"Ed25519","kid":"test","alg":"EdDSA","x":"IoVG8onMfMsFo4z8KXavXa1EWf2IhAGV1INRF37YjpA"}]}"#;
const TEST_JWT: &str = "eyJhbGciOiJFZERTQSIsImtpZCI6InRlc3QifQ.eyJzdWIiOiJpbnRlZ3JhdGlvbi10ZXN0IiwiaXNzIjoidGVzdC1pc3N1ZXIiLCJleHAiOjQxMDI0NDQ4MDB9.ImTw3cMdfuL2S4qUF9vA_ncYzNAdY6Y_24Qkp7oz1Lb55uW9Kl0c6uAVfIPybjEiOoojO08NtzaiBnZNbLJ1AA";
const TEST_JWT_HEADER: &str = "eyJhbGciOiJFZERTQSIsImtpZCI6InRlc3QifQ";
const TEST_JWT_PAYLOAD: &str = "eyJzdWIiOiJpbnRlZ3JhdGlvbi10ZXN0IiwiaXNzIjoidGVzdC1pc3N1ZXIiLCJleHAiOjQxMDI0NDQ4MDB9";

struct TempDir(std::path::PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn placeholder_is_selected_only_without_jwks() {
    let options = opensovd_gateway::cli::try_parse_from(["gateway"]).expect("options");
    assert!(auth::configured(&options).is_some());
    let jwt_options = opensovd_gateway::cli::try_parse_from([
        "gateway",
        "--auth-jwks",
        "jwks.json",
        "--auth-issuer",
        "test-issuer",
    ])
    .expect("JWT options");
    assert!(auth::configured(&jwt_options).is_none());
}
