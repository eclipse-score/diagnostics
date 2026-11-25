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

/// Writes a CDA configuration that serves on `socket`. `doip` is the body of the `[doip]`
/// section and `init_mode` the diagnostic communication mode.
fn write_config(path: &Path, database: &Path, storage: &Path, socket: &Path, doip: &str, init_mode: &str) {
    let [database, storage, socket] = [database, storage, socket].map(|path| path.display().to_string());
    let config = format!(
        r#"[database]
dir = {database:?}
exit_no_database_loaded = true

[doip]
{doip}

[communication]
init_mode = "{init_mode}"

[runtime_update_config]
storage_dir = {storage:?}

[server]
transport = "unix_socket"
unix_socket = {socket:?}
"#
    );
    fs::write(path, config).expect("write CDA config");
}

/// The gateway's own listener still needs a concrete TCP port, so a free port is picked and released
/// before the gateway binds it. Another process can take it in between; the gateway then exits during
/// startup and is retried on a new port.
const START_ATTEMPTS: usize = 5;

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
    // authorize needs no token, so it tells when the gateway is up. The token it issues is the one
    // the gateway forwards to the CDA, which reads its `sub` and `exp`.
    let url = format!("http://{address}/sovd/v1");
    let anonymous = Client::builder()
        .base_uri(&url)
        .expect("gateway URL")
        .timeout(Duration::from_secs(5))
        .build()
        .expect("anonymous gateway client");
    let mut last_error = None;
    let mut issued = None;
    for _ in 0..120 {
        if let Some(status) = child.0.try_wait().expect("check gateway process") {
            eprintln!("gateway exited during startup: {status}");
            return None;
        }
        match anonymous.authorize("integration-test", "unchecked").await {
            Ok(response) => {
                issued = Some(response);
                break;
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let issued = issued.unwrap_or_else(|| panic!("gateway did not become reachable: {last_error:?}"));
    assert_eq!(issued.token_type, "Bearer", "{issued:?}");
    let token = issued.access_token;
    let value = http::HeaderValue::from_str(&format!("Bearer {token}")).expect("token header");
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
    // Keep diagnostic communication disabled: discovery here only needs the loaded databases.
    write_config(&config, &database, &storage, &socket.0, r#"tester_address = "127.0.0.1""#, "Disabled");
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
        .output()
        .expect("run example client against gateway");
    assert!(
        output.status.success(),
        "example client failed through gateway: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(gateway.0.try_wait().expect("check gateway process").is_none());
}

/// The pinned Core client has no generic POST or DELETE. Use curl only for those
/// methods, through the same gateway and bearer token; all reads use the Core client.
#[cfg(feature = "ecu_simulator")]
fn write_request(url: &str, token: &str, method: &str, body: Option<&serde_json::Value>, expected: u16) -> String {
    assert!(matches!(method, "POST" | "DELETE"));
    let mut command = Command::new("curl");
    command.args([
        "--silent", "--show-error", "--max-time", "10", "--request", method,
        "--header", &format!("Authorization: Bearer {token}"),
        "--header", "Content-Type: application/json", "--write-out", "\n%{http_code}",
    ]);
    if let Some(body) = body {
        command.args(["--data-raw", &body.to_string()]);
    }
    let output = command.arg(url).output().expect("run curl for test-only write request");
    assert!(output.status.success(), "curl failed: {}", String::from_utf8_lossy(&output.stderr));
    let response = String::from_utf8(output.stdout).expect("UTF-8 response");
    let (body, status) = response.rsplit_once('\n').expect("curl status");
    assert_eq!(status.parse::<u16>().expect("HTTP status"), expected, "{method} {url}: {body}");
    body.to_owned()
}

/// Calls the simulator's control API for the FLXC1000 ECU. This is fixture setup and
/// inspection of the simulated ECU, not an SOVD request.
#[cfg(feature = "ecu_simulator")]
fn simulator_control(control: &str, method: &str, path: &str, body: Option<&serde_json::Value>) -> String {
    let url = format!("{control}/FLXC1000{path}");
    let mut command = Command::new("curl");
    command.args(["--silent", "--show-error", "--fail", "--max-time", "10", "--request", method]);
    if let Some(body) = body {
        command.args(["--header", "Content-Type: application/json", "--data-raw", &body.to_string()]);
    }
    let output = command.arg(&url).output().expect("run curl for simulator control request");
    assert!(output.status.success(), "{method} {url}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("UTF-8 simulator response")
}

/// Reads a JSON resource of the simulator's control API.
#[cfg(feature = "ecu_simulator")]
fn simulator_state(control: &str, path: &str) -> serde_json::Value {
    let body = simulator_control(control, "GET", path, None);
    serde_json::from_str(&body).unwrap_or_else(|error| panic!("simulator {path} is not JSON ({error}): {body}"))
}

/// Checks that the simulated ECU received the UDS requests of every step, so a run
/// that never reached the ECU cannot pass.
#[cfg(feature = "ecu_simulator")]
fn assert_recorded_requests(recorded: &serde_json::Value) {
    let frames: Vec<String> = recorded
        .as_array()
        .expect("recorded requests")
        .iter()
        .map(|frame| frame.as_str().expect("recorded request").to_ascii_lowercase())
        .collect();
    // VIN read, clear all DTCs, routine stop, extended session, hard reset.
    for exact in ["22f190", "14ffffff", "31021003", "1003", "1101"] {
        assert!(frames.iter().any(|frame| frame == exact), "missing {exact}: {frames:?}");
    }
    // DTC list, snapshot and extended data reads, routine start.
    for prefix in ["1902", "1904", "1906", "31011003"] {
        assert!(frames.iter().any(|frame| frame.starts_with(prefix)), "missing {prefix}: {frames:?}");
    }
    // One RequestResults per observed result step.
    let results = frames.iter().filter(|frame| *frame == "31031003").count();
    assert_eq!(results, 4, "RequestResults count: {frames:?}");
}

/// Verifies live identification, fault details and clearing, routine results and
/// stop, and ECU reset through the gateway and CDA against the simulated ECU, and
/// checks the requests and final state the simulator recorded. Requires the
/// separately provisioned simulator (`tests/ecu_sim_ci.sh`).
// # verifies: comp_req__sovd_gateway__single_entry_point, comp_req__sovd_gateway__forwarding, comp_req__cda__faults, comp_req__cda__operations, comp_req__cda__lock_api, comp_req__cda__lock_enforcement, comp_req__cda__resource_mapping
#[cfg(feature = "ecu_simulator")]
#[tokio::test(flavor = "current_thread")]
async fn simulator_reads_through_gateway() {
    let tester_address = env::var("DIAGNOSTICS_TESTER_ADDRESS").expect("simulator tester address");
    let _: std::net::Ipv4Addr = tester_address.parse().expect("IPv4 tester address");
    let control = env::var("DIAGNOSTICS_SIMULATOR_CONTROL").expect("simulator control API URL");
    // Values follow the pinned CDA's integration-tests/tests/sovd/faults.rs (Apache-2.0).
    let seed = serde_json::json!({
        "id": "01E240", "statusMask": "29", "emissionsRelated": false,
        "snapshots": [{"recordNumber": "FE", "records": [{"did": "19FE", "data": "10111213"}]}],
        "extendedData": [{"recordNumber": "FF", "data": "20212223"}],
    });
    simulator_control(&control, "PUT", "/dtc/Standard", Some(&seed));
    simulator_control(&control, "POST", "/record", None);
    let temp = test_dir();
    let database = temp.0.join("database");
    let storage = temp.0.join("storage");
    fs::create_dir_all(&database).expect("create database directory");
    fs::create_dir_all(&storage).expect("create storage directory");
    for (source, name) in [
        ("score/opensovd-cda/mdd/functional_groups.mdd", "functional_groups.mdd"),
        ("classic-diagnostic-adapter+/testcontainer/odx/FLXC1000.mdd", "FLXC1000.mdd"),
    ] {
        fs::copy(runfile(source), database.join(name)).expect("copy MDD fixture");
    }
    let socket = SocketFile::new();
    let config = temp.0.join("cda.toml");
    let doip = format!(
        r#"tester_address = {tester_address:?}
tester_subnet = "255.255.0.0""#
    );
    write_config(&config, &database, &storage, &socket.0, &doip, "Always");
    let (_cda, _) = start_cda(
        &runfile("classic-diagnostic-adapter+/cda-main/opensovd-cda"),
        &config,
        &socket.0,
    )
    .await
    .expect("CDA did not start");
    let gateway = runfile("score/opensovd-core/opensovd-gateway");
    let upstream = format!("unix:{}:/vehicle/v15", socket.0.display());
    let mut started = None;
    for _ in 0..START_ATTEMPTS {
        if let Some(gateway) = start_gateway(&gateway, &upstream).await {
            started = Some(gateway);
            break;
        }
    }
    let (_gateway, client, gateway_url, token) = started.expect("gateway did not start");
    let vin = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            match client.component("flxc1000").data("vindataidentifier").read().send().await {
                Ok(data) => break data,
                Err(error) => {
                    eprintln!("waiting for simulated ECU VIN: {error}");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    })
    .await
    .expect("simulated ECU VIN did not become available");
    assert_eq!(vin.data["VIN"].as_str(), Some("SCEDT26T8BD005261"), "{vin:?}");

    // The fixture seeds this fault in the simulator, not in the CDA's database.
    let faults: serde_json::Value = client.get("/components/flxc1000/faults", &[]).await.expect("read faults");
    let listed = faults["items"].as_array().expect("fault collection");
    assert!(listed.iter().any(|fault| fault["code"] == "01E240"), "{faults}");
    let detail: serde_json::Value = client
        .get("/components/flxc1000/faults/01E240", &[])
        .await
        .expect("read fault details");
    assert_eq!(detail["item"]["code"], "01E240", "{detail}");
    assert_eq!(detail["item"]["status"]["mask"], "29", "{detail}");
    assert_eq!(detail["item"]["status"]["test_failed"], true, "{detail}");
    let snapshots = &detail["environment_data"]["snapshots"]["data"]["FE"];
    assert_eq!(snapshots["DTCSnapshotRecordNumberOfIdentifiers"], 1, "{detail}");
    assert_eq!(snapshots["DTCSnapshotRecord"][0]["DTCSnapshotRecordDid"], 0x19FE, "{detail}");
    assert_eq!(snapshots["DTCSnapshotRecord"][0]["DTCSnapshotRecordDidData"], 0x10111213, "{detail}");
    assert_eq!(
        detail["environment_data"]["extended_data_records"]["data"]["FF"]["DefaultTestExtDataStruct"]
            ["DTCExtDataRecordData"],
        0x20212223,
        "{detail}"
    );

    let root = format!("{gateway_url}/components/flxc1000");
    let lock_url = format!("{root}/locks");
    write_request(&format!("{root}/faults"), &token, "DELETE", None, 409);
    let unchanged: serde_json::Value = client
        .get("/components/flxc1000/faults/01E240", &[])
        .await
        .expect("fault survives rejected clear");
    assert_eq!(unchanged["item"]["status"]["test_failed"], true, "{unchanged}");
    let lock_body = write_request(&lock_url, &token, "POST", Some(&serde_json::json!({"lock_expiration": 300})), 201);
    let lock: serde_json::Value = serde_json::from_str(&lock_body).expect("lock JSON");
    let lock_id = lock["id"].as_str().expect("lock id");
    let lock_path = format!("/components/flxc1000/locks/{lock_id}");
    let acquired: serde_json::Value = client.get(&lock_path, &[]).await.expect("read acquired lock");
    assert_eq!(acquired["owned"], true, "{acquired}");

    write_request(&format!("{root}/faults"), &token, "DELETE", None, 204);
    let cleared: serde_json::Value = client.get("/components/flxc1000/faults", &[]).await.expect("read cleared faults");
    assert!(
        !cleared["items"].as_array().expect("fault collection").iter().any(|fault| {
            fault["code"] == "01E240" && fault["status"]["test_failed"] == true
        }),
        "seeded fault still failed after clear: {cleared}"
    );

    // The pinned CDA's integration-tests/tests/sovd/operations.rs describes this simulator routine.
    let execution_path = "/components/flxc1000/operations/timecircuits/executions";
    let execution_url = format!("{gateway_url}{execution_path}");
    let started = write_request(
        &execution_url, &token, "POST",
        Some(&serde_json::json!({"parameters":{"travelMethod":"PresentDay","travelMethodData":{}}})), 202,
    );
    let started: serde_json::Value = serde_json::from_str(&started).expect("execution JSON");
    assert_eq!(started["status"], "running", "{started}");
    let id = started["id"].as_str().expect("execution id");
    let execution = format!("{execution_path}/{id}");
    let running: serde_json::Value = client
        .get(&execution, &[("x-sovd2uds-suppressService", "true")])
        .await
        .expect("read running execution");
    assert_eq!(running["status"], "running", "{running}");
    for (percent, step) in [(25, "Accelerating"), (50, "TemporalDisplacement"), (75, "Arrived"), (100, "Arrived")] {
        let result: serde_json::Value = client.get(&execution, &[]).await.expect("observe routine result");
        // This CDA marks every positive RequestResults response completed, even at 25%.
        assert_eq!(result["status"], "completed", "{result}");
        assert_eq!(result["parameters"]["percentComplete"], percent, "{result}");
        assert_eq!(result["parameters"]["step"], step, "{result}");
        if percent >= 75 {
            assert_eq!(result["parameters"]["message"], "Great Scott! We made it!", "{result}");
        }
    }
    let stopped = write_request(&format!("{gateway_url}{execution}"), &token, "DELETE", None, 200);
    let stopped: serde_json::Value = serde_json::from_str(&stopped).expect("stopped execution JSON");
    assert_eq!(stopped["status"], "stopped", "{stopped}");
    let error = client.get::<serde_json::Value>(&execution, &[]).await.expect_err("stopped execution removed");
    assert!(
        matches!(error, opensovd_client::Error::ApiError { status, .. } if status == http::StatusCode::NOT_FOUND),
        "{error:?}"
    );

    // A hard reset must actually change an extended session back to default.
    client.put("/components/flxc1000/modes/session", &[], &serde_json::json!({"value":"extended"}))
        .await.expect("enter extended session");
    let session: serde_json::Value = client
        .get("/components/flxc1000/modes/session", &[])
        .await
        .expect("read extended session");
    assert_eq!(session["value"].as_str().map(str::to_ascii_lowercase).as_deref(), Some("extended"), "{session}");
    write_request(
        &format!("{root}/operations/reset/executions"),
        &token,
        "POST",
        Some(&serde_json::json!({"parameters":{"value":"hardreset"}})),
        204,
    );
    let session: serde_json::Value = client
        .get("/components/flxc1000/modes/session", &[])
        .await
        .expect("read reset session");
    assert_eq!(session["value"].as_str().map(str::to_ascii_lowercase).as_deref(), Some("default"), "{session}");
    write_request(&format!("{lock_url}/{lock_id}"), &token, "DELETE", None, 204);

    // The simulated ECU itself must show the effect of every step.
    assert_recorded_requests(&simulator_state(&control, "/record"));
    let dtcs = simulator_state(&control, "/dtc/Standard");
    assert_eq!(dtcs, serde_json::json!([]), "fault memory not cleared");
    let state = simulator_state(&control, "/state");
    assert_eq!(state["sessionState"], "DEFAULT", "ECU state after reset: {state}");
}
