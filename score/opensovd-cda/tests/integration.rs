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

use opensovd_client::Client;
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

struct ChildGuard(Child);

/// Removes the per-test directory even when an assertion fails.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn test_dir() -> TempDir {
    let base = env::var_os("TEST_TMPDIR").map_or_else(env::temp_dir, PathBuf::from);
    let path = base.join(format!("inc-diagnostics-cda-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create test directory");
    TempDir(path)
}

/// A Unix socket path. Socket paths are limited to about 108 bytes, which the test directory under
/// Bazel's sandbox exceeds, so the socket lives in `/tmp` and is removed on drop.
struct SocketFile(PathBuf);

impl SocketFile {
    fn new() -> Self {
        let path = PathBuf::from("/tmp").join(format!("cda-{}.sock", std::process::id()));
        let _ = fs::remove_file(&path);
        Self(path)
    }
}

impl Drop for SocketFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn runfile(path: &str) -> PathBuf {
    let root = PathBuf::from(env::var_os("TEST_SRCDIR").expect("TEST_SRCDIR"));
    let workspace = env::var("TEST_WORKSPACE").expect("TEST_WORKSPACE");
    let candidates = [
        root.join(&workspace).join(path),
        root.join("_main").join(path),
        root.join(path),
    ];
    candidates
        .into_iter()
        .find(|candidate| candidate.exists())
        .unwrap_or_else(|| panic!("runfile not found: {path}"))
}

fn write_config(path: &Path, database: &Path, storage: &Path, socket: &Path) {
    let config = format!(
        "[database]\ndir = {:?}\nexit_no_database_loaded = true\n\n\
         [doip]\ntester_address = \"127.0.0.1\"\n\n\
         [runtime_update_config]\nstorage_dir = {:?}\n\n\
         [server]\ntransport = \"unix_socket\"\nunix_socket = {:?}\n",
        database.display().to_string(),
        storage.display().to_string(),
        socket.display().to_string(),
    );
    fs::write(path, config).expect("write CDA config");
}

/// The gateway's own listener still needs a concrete TCP port, so a free port is picked and released
/// before the gateway binds it. Another process can take it in between; the gateway then exits during
/// startup and is retried on a new port.
const START_ATTEMPTS: usize = 5;
// Test-only Ed25519 public key and JWT, signed for test-issuer and expiring in 2100.
const TEST_JWKS: &str = r#"{"keys":[{"kty":"OKP","crv":"Ed25519","kid":"test","alg":"EdDSA","x":"IoVG8onMfMsFo4z8KXavXa1EWf2IhAGV1INRF37YjpA"}]}"#;
const TEST_JWT: &str = "eyJhbGciOiJFZERTQSIsImtpZCI6InRlc3QifQ.eyJzdWIiOiJpbnRlZ3JhdGlvbi10ZXN0IiwiaXNzIjoidGVzdC1pc3N1ZXIiLCJleHAiOjQxMDI0NDQ4MDB9.ImTw3cMdfuL2S4qUF9vA_ncYzNAdY6Y_24Qkp7oz1Lb55uW9Kl0c6uAVfIPybjEiOoojO08NtzaiBnZNbLJ1AA";

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("pick test port")
        .port()
}

/// Starts the gateway forwarding to the CDA at `upstream`. Returns `None` if it exited during startup.
async fn start_gateway(binary: &Path, upstream: &str, jwks: &Path) -> Option<(ChildGuard, Client, String)> {
    let address = format!("127.0.0.1:{}", free_port());
    let mut child = ChildGuard(
        Command::new(binary)
            .env("SOVD_URL", format!("http://{address}/sovd"))
            .env("SOVD_UPSTREAM_URL", upstream)
            .env_remove("SOVD_AUTH_TOKEN")
            .arg("--auth-jwks")
            .arg(jwks)
            .args(["--auth-issuer", "test-issuer"])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn gateway"),
    );
    let url = format!("http://{address}/sovd/v1");
    let client = Client::builder()
        .base_uri(&url)
        .expect("gateway URL")
        .timeout(Duration::from_secs(5))
        .layer(tower::util::MapRequestLayer::new(|mut request: http::Request<_>| {
            request.headers_mut().insert(
                http::header::AUTHORIZATION,
                http::HeaderValue::from_str(&format!("Bearer {TEST_JWT}")).expect("test token header"),
            );
            request
        }))
        .build()
        .expect("gateway client");
    let mut last_error = None;
    for _ in 0..120 {
        if let Some(status) = child.0.try_wait().expect("check gateway process") {
            eprintln!("gateway exited during startup: {status}");
            return None;
        }
        match client.list_apps().send().await {
            Ok(_) => return Some((child, client, url)),
            Err(error) => last_error = Some(error.to_string()),
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("gateway did not become reachable: {last_error:?}");
}

#[test]
fn gateway_refuses_to_start_without_authentication() {
    let mut child = ChildGuard(
        Command::new(runfile("score/opensovd-core/opensovd-gateway"))
            .env_remove("SOVD_AUTH_TOKEN")
            .env_remove("SOVD_AUTH_JWKS")
            .env_remove("SOVD_AUTH_ISSUER")
            .env_remove("SOVD_AUTH_AUDIENCE")
            .env_remove("SOVD_AUTH_POLICY")
            .env_remove("SOVD_AUTH_POLICY_DATA")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run gateway"),
    );
    let mut status = None;
    for _ in 0..500 {
        status = child.0.try_wait().expect("check gateway process");
        if status.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!status.expect("gateway did not exit without authentication").success());
    let mut logs = String::new();
    child.0.stdout.take().expect("stdout").read_to_string(&mut logs).expect("read stdout");
    child.0.stderr.take().expect("stderr").read_to_string(&mut logs).expect("read stderr");
    assert!(
        logs.contains("configure SOVD_AUTH_TOKEN or --auth-jwks and --auth-issuer"),
        "{logs}"
    );
}

/// Starts the CDA and waits until it serves `/components` on its Unix socket. Returns `None` if it
/// exited during startup.
async fn start_cda(binary: &Path, config: &Path, socket: &Path) -> Option<(ChildGuard, Client)> {
    let mut child = ChildGuard(
        Command::new(binary)
            .arg("--config")
            .arg(config)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn CDA"),
    );
    let client = Client::builder()
        .base_uri("http://localhost/vehicle/v15")
        .expect("CDA URL")
        .unix_socket(socket)
        .timeout(Duration::from_secs(1))
        .build()
        .expect("CDA client");
    let mut last_error = None;
    for _ in 0..120 {
        if let Some(status) = child.0.try_wait().expect("check CDA process") {
            eprintln!("CDA exited during startup: {status}");
            return None;
        }
        match client.list_components().send().await {
            Ok(components) => {
                assert!(components.items.is_empty());
                return Some((child, client));
            },
            Err(error) => last_error = Some(error.to_string()),
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("CDA did not become reachable: {last_error:?}");
}

// # verifies: comp_req__sovd_gateway__single_entry_point, comp_req__sovd_gateway__forwarding, comp_req__sovd_gateway__merged_discovery, comp_req__sovd_gateway__reference_rewrite, comp_req__cda__http_server_port
#[tokio::test(flavor = "current_thread")]
async fn cda_serves_version_and_components() {
    let binary = runfile("classic-diagnostic-adapter+/cda-main/opensovd-cda");
    let mdd = runfile("score/opensovd-cda/mdd/functional_groups.mdd");
    let temp = test_dir();
    let jwks = temp.0.join("jwks.json");
    fs::write(&jwks, TEST_JWKS).expect("write JWKS");
    let database = temp.0.join("database");
    fs::create_dir_all(&database).expect("create database directory");
    fs::copy(&mdd, database.join("functional_groups.mdd")).expect("copy MDD fixture");
    let config = temp.0.join("cda.toml");
    let storage = temp.0.join("storage");
    fs::create_dir_all(&storage).expect("create storage directory");

    let socket = SocketFile::new();
    write_config(&config, &database, &storage, &socket.0);
    let (mut child, client) = start_cda(&binary, &config, &socket.0).await.expect("CDA did not start");
    let upstream = format!("unix:{}:/vehicle/v15", socket.0.display());

    assert!(child.0.try_wait().expect("check CDA process").is_none());
    let apps = client.list_apps().send().await.expect("discover CDA apps");
    assert!(apps.items.iter().any(|app| app.id == "sovd2uds"));
    let network = client
        .app("sovd2uds")
        .data("networkstructure")
        .read()
        .send()
        .await
        .expect("read fixture network structure");
    let groups = network.data[0]["FunctionalGroups"]
        .as_array()
        .expect("functional groups collection");
    assert!(
        !groups.is_empty(),
        "MDD fixture must expose functional groups: {}",
        network.data
    );
    let example = runfile("score/examples/sovd-client/sovd-client");

    // UC02/UC06 through the gateway: the CDA's entities are discovered next to the gateway's own
    // and their data is read through the gateway, with references pointing at the gateway.
    let gateway = runfile("score/opensovd-core/opensovd-gateway");
    let mut started = None;
    for _ in 0..START_ATTEMPTS {
        if let Some(gateway) = start_gateway(&gateway, &upstream, &jwks).await {
            started = Some(gateway);
            break;
        }
    }
    let (mut gateway, gateway_client, gateway_url) = started.expect("gateway did not start on any free port");
    let apps = gateway_client
        .list_apps()
        .send()
        .await
        .expect("discover apps through gateway");
    assert!(apps.items.iter().any(|app| app.id == "sovd2uds"));
    assert!(apps.items.iter().any(|app| app.id == "score-diagnostics"));
    let listing = format!("{:?}", apps.items);
    assert!(
        !listing.contains(&socket.0.display().to_string()),
        "upstream socket leaked through gateway: {listing}"
    );
    let forwarded = gateway_client
        .app("sovd2uds")
        .data("networkstructure")
        .read()
        .send()
        .await
        .expect("read CDA data through gateway");
    assert_eq!(forwarded.data, network.data);
    let local = gateway_client
        .app("score-diagnostics")
        .data("application.version")
        .read()
        .send()
        .await
        .expect("read gateway data");
    assert_eq!(local.data["value"].as_str(), Some("1.0.0"));
    let output = Command::new(&example)
        .arg(&gateway_url)
        .env("SOVD_AUTH_TOKEN", TEST_JWT)
        .output()
        .expect("run example client against gateway");
    assert!(
        output.status.success(),
        "example client failed through gateway: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(gateway.0.try_wait().expect("check gateway process").is_none());
}
