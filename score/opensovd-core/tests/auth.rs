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

async fn request(address: SocketAddr, authorization: Option<&str>) -> String {
    let mut stream = TcpStream::connect(address)
        .await
        .expect("connect to gateway");
    let header = authorization.map_or(String::new(), |value| format!("Authorization: {value}\r\n"));
    let request = format!(
        "GET /sovd/v1/functions HTTP/1.1\r\nHost: localhost\r\n{header}Connection: close\r\n\r\n"
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

#[tokio::test(flavor = "current_thread")]
async fn authentication_precedes_forwarding() {
    let dir = std::env::var_os("TEST_TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("gateway-jwt-forwarding-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("test directory");
    let _dir = TempDir(dir.clone());
    let jwks = dir.join("jwks.json");
    std::fs::write(&jwks, TEST_JWKS).expect("JWKS");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("upstream listener");
    let upstream_addr = listener.local_addr().expect("upstream address");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let _upstream = TaskGuard(tokio::spawn(async move {
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

    // The Core runner binds from its URL; retry if another process takes the port.
    let mut gateway = None;
    for _ in 0..5 {
        let reservation = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve port");
        let address = reservation.local_addr().expect("gateway address");
        let url = format!("http://{address}/sovd");
        let upstream = format!("http://{upstream_addr}/vehicle/v15");
        let options = opensovd_gateway::cli::try_parse_from([
            "gateway",
            "--url",
            &url,
            "--upstream-url",
            &upstream,
            "--auth-jwks",
            jwks.to_str().expect("JWKS path"),
            "--auth-issuer",
            "test-issuer",
        ])
        .expect("options");
        assert!(auth::configured(&options, None)
            .expect("configuration")
            .is_none());
        drop(reservation);
        let task = TaskGuard(tokio::spawn(opensovd_gateway::run_with_shutdown(
            options,
            Topology::default(),
            std::future::pending(),
        )));
        for _ in 0..100 {
            if task.0.is_finished() {
                break;
            }
            if TcpStream::connect(address).await.is_ok() {
                gateway = Some((task, address));
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        if gateway.is_some() {
            break;
        }
    }
    let (_gateway, address) = gateway.expect("gateway started");
    let rejected = request(address, None).await;
    assert!(rejected.starts_with("HTTP/1.1 401"), "{rejected}");
    assert!(requests.lock().expect("requests").is_empty());

    let invalid = request(address, Some("Bearer wrong")).await;
    assert!(invalid.starts_with("HTTP/1.1 401"), "{invalid}");
    assert!(requests.lock().expect("requests").is_empty());

    let accepted = request(address, Some(&format!("Bearer {TEST_JWT}"))).await;
    assert!(accepted.starts_with("HTTP/1.1 200"), "{accepted}");
    assert!(accepted.ends_with("ok"), "{accepted}");
    let requests = requests.lock().expect("requests");
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].starts_with("GET /vehicle/v15/functions "),
        "{}",
        requests[0]
    );
    assert!(requests[0].lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("authorization")
                && value.trim() == format!("Bearer {TEST_JWT}")
        })
    }));
}

// Test-only Ed25519 public key and JWT, signed for test-issuer and expiring in 2100.
const TEST_JWKS: &str = r#"{"keys":[{"kty":"OKP","crv":"Ed25519","kid":"test","alg":"EdDSA","x":"IoVG8onMfMsFo4z8KXavXa1EWf2IhAGV1INRF37YjpA"}]}"#;
const TEST_JWT: &str = "eyJhbGciOiJFZERTQSIsImtpZCI6InRlc3QifQ.eyJzdWIiOiJpbnRlZ3JhdGlvbi10ZXN0IiwiaXNzIjoidGVzdC1pc3N1ZXIiLCJleHAiOjQxMDI0NDQ4MDB9.ImTw3cMdfuL2S4qUF9vA_ncYzNAdY6Y_24Qkp7oz1Lb55uW9Kl0c6uAVfIPybjEiOoojO08NtzaiBnZNbLJ1AA";

struct TempDir(std::path::PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn jwt_and_permissive_rego_require_a_principal_before_forwarding() {
    let dir = std::env::var_os("TEST_TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(format!("gateway-jwt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("test directory");
    let _dir = TempDir(dir.clone());
    let jwks = dir.join("jwks.json");
    let policy = dir.join("allow.rego");
    std::fs::write(&jwks, TEST_JWKS).expect("JWKS");
    std::fs::write(&policy, "package sovd.authz\nimport rego.v1\nallow := true\n")
        .expect("policy");

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("upstream listener");
    let upstream = format!("http://{}/vehicle/v15", listener.local_addr().expect("address"));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let _upstream = TaskGuard(tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buffer).await.expect("read upstream request");
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..n]);
            }
            recorded.lock().expect("requests").push(request);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .expect("upstream response");
        }
    }));

    let reservation = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve port");
    let address = reservation.local_addr().expect("gateway address");
    let url = format!("http://{address}/sovd");
    let options = opensovd_gateway::cli::try_parse_from([
        "gateway",
        "--url",
        &url,
        "--upstream-url",
        &upstream,
        "--auth-jwks",
        jwks.to_str().expect("JWKS path"),
        "--auth-issuer",
        "test-issuer",
        "--auth-policy",
        policy.to_str().expect("policy path"),
    ])
    .expect("options");
    drop(reservation);
    // The normal runner constructs JWT and Rego through create_auth, not supplied static auth.
    let gateway = TaskGuard(tokio::spawn(opensovd_gateway::run_with_shutdown(
        options,
        Topology::default(),
        std::future::pending(),
    )));
    for _ in 0..100 {
        assert!(!gateway.0.is_finished(), "gateway exited during startup");
        if TcpStream::connect(address).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let rejected = request(address, None).await;
    assert!(rejected.starts_with("HTTP/1.1 401"), "{rejected}");
    assert!(requests.lock().expect("requests").is_empty());

    let accepted = request(address, Some(&format!("Bearer {TEST_JWT}"))).await;
    assert!(accepted.starts_with("HTTP/1.1 200"), "{accepted}");
    assert!(accepted.ends_with("ok"), "{accepted}");
    let requests = requests.lock().expect("requests");
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with(b"GET /vehicle/v15/functions "));
    let forwarded = std::str::from_utf8(&requests[0]).expect("upstream request");
    assert!(forwarded.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case("authorization") && value.trim() == format!("Bearer {TEST_JWT}")
        })
    }));
}

#[test]
fn missing_or_empty_configuration_is_rejected() {
    let options = opensovd_gateway::cli::try_parse_from(["gateway"]).expect("options");
    assert!(auth::configured(&options, None).is_err());
    assert!(auth::configured(&options, Some(String::new())).is_err());
    assert!(auth::configured(&options, Some(" ".into())).is_err());
    assert!(auth::configured(&options, Some("a=b".into())).is_err());
    assert!(auth::configured(&options, Some("==".into())).is_err());
    assert!(auth::configured(&options, Some("test-token".into()))
        .expect("static auth")
        .is_some());
    let jwt_options = opensovd_gateway::cli::try_parse_from([
        "gateway",
        "--auth-jwks",
        "jwks.json",
        "--auth-issuer",
        "test-issuer",
    ])
    .expect("JWT options");
    assert!(auth::configured(&jwt_options, Some("test-token".into())).is_err());
}
