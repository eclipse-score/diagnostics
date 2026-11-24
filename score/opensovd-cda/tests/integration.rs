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

use opensovd_client::{Client, VendorInfo};
use std::{
    env, fs,
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
    // Keep diagnostic communication disabled: discovery here only needs the loaded databases.
    let config = format!(
        "[database]\ndir = {:?}\nexit_no_database_loaded = true\n\n\
         [doip]\ntester_address = \"127.0.0.1\"\n\n\
         [communication]\ninit_mode = \"Disabled\"\n\n\
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

/// Sends one HTTP/1.1 request to the gateway at `address` and returns the status line and the body.
async fn http(address: &str, method: &str, path: &str, body: &str) -> Option<(String, String)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(address).await.ok()?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.ok()?;
    let response = String::from_utf8(response).ok()?;
    let (head, body) = response.split_once("\r\n\r\n")?;
    Some((head.lines().next()?.to_owned(), body.to_owned()))
}

/// Gets a token from the `authorize` of the gateway at `address`, as a client does before its first
/// request. The token is the one the gateway forwards to the CDA, which reads its `sub` and `exp`.
async fn gateway_token(address: &str) -> String {
    let (status, body) = http(
        address,
        "POST",
        "/sovd/v1/authorize",
        r#"{"client_id":"integration-test","client_secret":"unchecked"}"#,
    )
    .await
    .expect("request a token from the gateway");
    assert!(status.contains(" 200"), "authorize: {status} {body}");
    let body: serde_json::Value = serde_json::from_str(&body).expect("authorize body");
    assert_eq!(body["token_type"], "Bearer", "{body}");
    body["access_token"].as_str().expect("access_token").to_owned()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("pick test port")
        .port()
}

/// Starts the gateway forwarding to the CDA at `upstream`. Returns `None` if it exited during startup.
/// The client sends a token that the gateway issued, to the gateway and, forwarded, to the CDA.
async fn start_gateway(binary: &Path, upstream: &str) -> Option<(ChildGuard, Client, String, String)> {
    let address = format!("127.0.0.1:{}", free_port());
    let mut child = ChildGuard(
        Command::new(binary)
            .env("SOVD_URL", format!("http://{address}/sovd"))
            .env("SOVD_UPSTREAM_URL", upstream)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn gateway"),
    );
    // version-info needs no token, so it tells when the gateway is up.
    let mut last_status = None;
    let mut up = false;
    for _ in 0..120 {
        if let Some(status) = child.0.try_wait().expect("check gateway process") {
            eprintln!("gateway exited during startup: {status}");
            return None;
        }
        match http(&address, "GET", "/sovd/version-info", "").await {
            Some((status, _)) if status.contains(" 200") => {
                up = true;
                break;
            }
            other => last_status = other,
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(up, "gateway did not become reachable: {last_status:?}");
    let token = gateway_token(&address).await;
    let value = http::HeaderValue::from_str(&format!("Bearer {token}")).expect("token header");
    let url = format!("http://{address}/sovd/v1");
    let client = Client::builder()
        .base_uri(&url)
        .expect("gateway URL")
        .timeout(Duration::from_secs(5))
        .layer(tower::util::MapRequestLayer::new(move |mut request: http::Request<_>| {
            request.headers_mut().insert(http::header::AUTHORIZATION, value.clone());
            request
        }))
        .build()
        .expect("gateway client");
    client.list_apps().send().await.expect("list apps with the issued token");
    Some((child, client, url, token))
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
            Ok(components) if components.items.iter().any(|ecu| ecu.id == "flxc1000") => {
                return Some((child, client));
            }
            Ok(_) => last_error = Some("fixture ECU not yet listed".into()),
            Err(error) => last_error = Some(error.to_string()),
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("CDA did not become reachable: {last_error:?}");
}

// # verifies: comp_req__sovd_gateway__single_entry_point, comp_req__sovd_gateway__forwarding, comp_req__sovd_gateway__merged_discovery, comp_req__sovd_gateway__reference_rewrite
#[tokio::test(flavor = "current_thread")]
async fn cda_serves_version_and_components() {
    let binary = runfile("classic-diagnostic-adapter+/cda-main/opensovd-cda");
    let mdd = runfile("score/opensovd-cda/mdd/functional_groups.mdd");
    let temp = test_dir();
    let database = temp.0.join("database");
    fs::create_dir_all(&database).expect("create database directory");
    fs::copy(&mdd, database.join("functional_groups.mdd")).expect("copy MDD fixture");
    // CDA lists loaded ECUs even when they are offline; no simulator or bus request is needed.
    let ecu_mdd = runfile("classic-diagnostic-adapter+/testcontainer/odx/FLXC1000.mdd");
    fs::copy(ecu_mdd, database.join("FLXC1000.mdd")).expect("copy ECU MDD fixture");
    let config = temp.0.join("cda.toml");
    let storage = temp.0.join("storage");
    fs::create_dir_all(&storage).expect("create storage directory");

    let socket = SocketFile::new();
    write_config(&config, &database, &storage, &socket.0);
    let (mut child, client) = start_cda(&binary, &config, &socket.0).await.expect("CDA did not start");
    let upstream = format!("unix:{}:/vehicle/v15", socket.0.display());

    assert!(child.0.try_wait().expect("check CDA process").is_none());
    // CDA's unversioned root is not forwarded by the versioned upstream proxy.
    let cda_versions = Client::builder()
        .base_uri("http://localhost")
        .expect("CDA discovery URL")
        .unix_socket(&socket.0)
        .timeout(Duration::from_secs(5))
        .discovery()
        .expect("CDA discovery client")
        .versions::<VendorInfo>()
        .await
        .expect("read CDA version-info");
    assert!(
        cda_versions.iter().any(|version| version.version == "1.1.0"
            && version.base_uri.0 == "/vehicle/v15"
            && version.vendor_info.as_ref().is_some_and(|vendor| {
                vendor.name == "Eclipse OpenSOVD Classic Diagnostic Adapter" && !vendor.version.is_empty()
            })),
        "CDA version-info: {cda_versions:?}"
    );
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
        if let Some(gateway) = start_gateway(&gateway, &upstream).await {
            started = Some(gateway);
            break;
        }
    }
    let (mut gateway, _startup_client, gateway_url, token) = started.expect("gateway did not start on any free port");
    let gateway_root = gateway_url.strip_suffix("/v1").expect("gateway versioned URL");
    // The discovery client sends no token: version-info is read before a client has one.
    let gateway_discovery = Client::builder()
        .base_uri(gateway_root)
        .expect("gateway discovery URL")
        .timeout(Duration::from_secs(5))
        .discovery()
        .expect("gateway discovery client");
    let gateway_versions = gateway_discovery
        .versions::<VendorInfo>()
        .await
        .expect("read gateway version-info");
    assert!(
        gateway_versions
            .iter()
            .any(|version| { version.version == "1.1.0" && version.base_uri.0 == gateway_url }),
        "gateway version-info: {gateway_versions:?}"
    );
    // The client of the advertised version sends the token on the requests that need one.
    let token_header = http::HeaderValue::from_str(&format!("Bearer {token}")).expect("token header");
    let gateway_client = Client::builder()
        .base_uri(gateway_root)
        .expect("gateway discovery URL")
        .timeout(Duration::from_secs(5))
        .layer(tower::util::MapRequestLayer::new(move |mut request: http::Request<_>| {
            request
                .headers_mut()
                .insert(http::header::AUTHORIZATION, token_header.clone());
            request
        }))
        .discovery()
        .expect("gateway discovery client with token")
        .select::<VendorInfo>(|version| version.version == "1.1.0" && version.base_uri.0 == gateway_url)
        .await
        .expect("select the gateway's advertised version");

    let anonymous = Client::builder()
        .base_uri(&gateway_url)
        .expect("gateway URL")
        .timeout(Duration::from_secs(5))
        .build()
        .expect("anonymous client");
    let error = anonymous
        .list_components()
        .send()
        .await
        .expect_err("anonymous request must fail");
    assert!(
        matches!(error, opensovd_client::Error::ApiError { status, .. } if status == http::StatusCode::UNAUTHORIZED),
        "expected 401: {error:?}"
    );

    // The CDA's data is not reachable through the gateway without a token either.
    let error = anonymous
        .app("sovd2uds")
        .data("networkstructure")
        .read()
        .send()
        .await
        .expect_err("anonymous CDA data request must fail");
    assert!(
        matches!(error, opensovd_client::Error::ApiError { status, .. } if status == http::StatusCode::UNAUTHORIZED),
        "expected 401: {error:?}"
    );
    // A malformed token is rejected by the gateway and never reaches the CDA.
    let malformed = Client::builder()
        .base_uri(&gateway_url)
        .expect("gateway URL")
        .timeout(Duration::from_secs(5))
        .layer(tower::util::MapRequestLayer::new(|mut request: http::Request<_>| {
            request.headers_mut().insert(
                http::header::AUTHORIZATION,
                http::HeaderValue::from_static("Bearer not-a-jwt"),
            );
            request
        }))
        .build()
        .expect("client with malformed token");
    let error = malformed
        .app("sovd2uds")
        .data("networkstructure")
        .read()
        .send()
        .await
        .expect_err("malformed token must fail");
    assert!(
        matches!(error, opensovd_client::Error::ApiError { status, .. } if status == http::StatusCode::UNAUTHORIZED),
        "expected 401: {error:?}"
    );

    let cda_ecus = client.list_components().send().await.expect("list CDA ECUs");
    let gateway_ecus = gateway_client
        .list_components()
        .send()
        .await
        .expect("list ECUs through gateway");
    assert!(
        cda_ecus.items.iter().any(|ecu| ecu.id == "flxc1000"),
        "fixture ECU missing: {cda_ecus:?}"
    );
    for ecu in &cda_ecus.items {
        let forwarded = gateway_ecus
            .items
            .iter()
            .find(|item| item.id == ecu.id)
            .unwrap_or_else(|| panic!("CDA ECU {} missing through gateway: {gateway_ecus:?}", ecu.id));
        let href = &forwarded.href.0;
        let path = href
            .strip_prefix(&gateway_url)
            .unwrap_or_else(|| panic!("ECU href does not point through gateway: {href}"));
        assert!(path.starts_with("/components/"), "unexpected ECU href: {href}");
        let actual: serde_json::Value = gateway_client
            .get(path, &[])
            .await
            .expect("resolve ECU href through gateway");
        assert_eq!(
            actual["id"].as_str(),
            Some(ecu.id.as_str()),
            "ECU href returned wrong resource: {actual}"
        );
    }
    let apps = gateway_client
        .list_apps()
        .send()
        .await
        .expect("discover apps through gateway");
    assert!(apps.items.iter().any(|app| app.id == "sovd2uds"));
    assert!(apps.items.iter().any(|app| app.id == "score-diagnostics"));
    let cda_app = apps
        .items
        .iter()
        .find(|app| app.id == "sovd2uds")
        .expect("CDA app through gateway");
    assert_eq!(
        cda_app.href.0,
        format!("{gateway_url}/apps/sovd2uds"),
        "CDA app reference must be rewritten to the gateway URL"
    );
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
    let cda_with = |token: String| {
        let value = http::HeaderValue::from_str(&format!("Bearer {token}")).expect("token header");
        Client::builder()
            .base_uri("http://localhost/vehicle/v15")
            .expect("CDA URL")
            .unix_socket(&socket.0)
            .timeout(Duration::from_secs(5))
            .layer(tower::util::MapRequestLayer::new(move |mut request: http::Request<_>| {
                request.headers_mut().insert(http::header::AUTHORIZATION, value.clone());
                request
            }))
            .build()
            .expect("CDA client with token")
    };
    // `networkstructure` needs no token at the CDA, but the locks of an ECU do: the CDA reads the
    // `sub` of the token for them.
    let locks = "/components/flxc1000/locks";
    let _: serde_json::Value = cda_with(token.clone())
        .get(locks, &[])
        .await
        .expect("CDA accepts the token of the gateway");
    let through_gateway: serde_json::Value = gateway_client
        .get(locks, &[])
        .await
        .expect("CDA accepts the token of the gateway through the gateway");
    assert!(through_gateway.is_object(), "{through_gateway}");
    let error = cda_with("aaaa.bbbb.cccc".to_owned())
        .get::<serde_json::Value>(locks, &[])
        .await
        .expect_err("CDA rejects an undecodable token");
    assert!(
        matches!(error, opensovd_client::Error::ApiError { status, .. } if status == http::StatusCode::FORBIDDEN),
        "expected 403 from the CDA: {error:?}"
    );
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
        .env("SOVD_AUTH_TOKEN", &token)
        .output()
        .expect("run example client against gateway");
    assert!(
        output.status.success(),
        "example client failed through gateway: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(gateway.0.try_wait().expect("check gateway process").is_none());
}
