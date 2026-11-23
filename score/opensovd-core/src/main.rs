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

//! The OpenSOVD gateway of S-CORE: the gateway of opensovd-core with the entities of S-CORE.
//!
//! It takes the options and `SOVD_*` environment variables of the reference gateway
//! (`opensovd-gateway --help` of opensovd-core), such as `SOVD_URL`, `SOVD_UPSTREAM_URL` for the CDA
//! and `SOVD_MDNS` for the mDNS/DNS-SD announcement. Upstreams, authentication, TLS, mDNS and
//! shutdown on SIGTERM or Ctrl-C are the ones of opensovd-core.

use opensovd_core::{App, Component, Topology};
use opensovd_models::data::DataCategory;
use opensovd_providers::data::{Constant, DataProviderBuilder};
use std::process::ExitCode;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[path = "../bridge/read_did.rs"]
mod read_did;

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

// # req-Id: comp_req__sovd_gateway__hosted_entities, comp_req__uds_data_bridge__read_handlers, comp_req__uds_data_bridge__utf8_data
async fn topology() -> Result<Topology, Box<dyn std::error::Error>> {
    let provider = DataProviderBuilder::new()
        .read_data(
            "demo.version",
            "Demo Version",
            &DataCategory::IdentData,
            Constant::new("1.0.0")?,
        )
        .read_data(
            "demo.status",
            "Demo Status",
            &DataCategory::CurrentData,
            Constant::new("ready")?,
        )
        .build()?;

    let app_provider = DataProviderBuilder::new()
        .read_data(
            "application.version",
            "Application Version",
            &DataCategory::IdentData,
            read_did::Utf8(read_did::ReadDid::demo(0, 0, Duration::from_secs(2))?),
        )
        .build()?;

    let topology = Topology::new();
    {
        let mut registry = topology.write().await;
        registry.add_component(Component::new("score-demo", "SCORE Demo").with_data_provider(provider));
        registry.add_app(
            App::new("score-diagnostics", "SCORE Diagnostics")
                .with_component_id("score-demo")
                .with_data_provider(app_provider),
        );
    }
    Ok(topology)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let options = opensovd_gateway::cli::parse();
    init_tracing();
    let topology = match topology().await {
        Ok(topology) => topology,
        Err(error) => {
            tracing::error!("Cannot set up the topology: {error}");
            return ExitCode::FAILURE;
        },
    };
    match opensovd_gateway::run(options, topology).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("{error:#}");
            ExitCode::FAILURE
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opensovd_client::Client;

    #[test]
    fn default_url_uses_ipv4_loopback() {
        const CHILD: &str = "SCORE_GATEWAY_DEFAULT_URL_TEST";
        if std::env::var_os(CHILD).is_none() {
            for value in [None, Some("http://[::1]:9000/env")] {
                let mut command = std::process::Command::new(std::env::current_exe().expect("test executable"));
                command.args(["--exact", "tests::default_url_uses_ipv4_loopback"]);
                match value {
                    Some(url) => { command.env(CHILD, url).env("SOVD_URL", url); },
                    None => { command.env(CHILD, "default").env_remove("SOVD_URL"); },
                }
                let output = command.output().expect("run URL test");
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(output.status.success() && stdout.contains("1 passed"), "{stdout}");
            }
            return;
        }
        let options = opensovd_gateway::cli::try_parse_from(["opensovd-gateway"]).expect("options");
        let expected = std::env::var(CHILD).expect("test mode");
        let expected = if expected == "default" { "http://127.0.0.1:7690/sovd" } else { &expected };
        assert_eq!(options.url, expected);
        let options = opensovd_gateway::cli::try_parse_from([
            "opensovd-gateway",
            "--url",
            "http://[::1]:8000/custom",
        ])
        .expect("explicit URL");
        assert_eq!(options.url, "http://[::1]:8000/custom");
    }

    // # verifies: comp_req__sovd_gateway__upstream_config
    #[test]
    fn upstream_configuration_accepts_mixed_urls_and_rejects_invalid_urls() {
        let parse = |urls| {
            opensovd_gateway::cli::try_parse_from(["opensovd-gateway", "--upstream-url", urls])
                .expect("options")
                .upstream
                .config()
        };
        let config = parse("http://localhost:20002/vehicle/v15,unix:///run/cda.sock:/vehicle/v15")
            .expect("mixed upstreams");
        assert_eq!(config.upstreams.len(), 2);
        assert!(parse("https://localhost:20002/vehicle/v15").is_err());
        assert!(parse("unix:relative.sock").is_err());
    }

    fn spawn_gateway(
        options: opensovd_gateway::cli::Cli,
        listener: tokio::net::TcpListener,
        topology: Topology,
    ) -> tokio::task::JoinHandle<Result<(), String>> {
        tokio::spawn(async move {
            opensovd_gateway::run_with_listener(options, topology, listener, std::future::pending())
                .await
                .map_err(|error| format!("{error:#}"))
        })
    }

    async fn wait_for_ready(
        task: &mut tokio::task::JoinHandle<Result<(), String>>,
        client: &Client,
    ) -> Result<(), String> {
        let ready = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let request = client.list_apps();
                tokio::select! {
                    result = &mut *task => return Err(format!("gateway exited before readiness: {result:?}")),
                    result = request.send() => if result.is_ok() { return Ok(()); },
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await;
        match ready {
            Ok(result) => result,
            Err(_) => {
                task.abort();
                Err("gateway did not become ready within 5 seconds".to_owned())
            },
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn gateway_startup_errors_are_reported() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let options = opensovd_gateway::cli::try_parse_from([
            "opensovd-gateway",
            "--url",
            "http://127.0.0.1:0/sovd",
            "--cors-origin",
            "*",
            "--cors-credentials",
        ])
        .expect("options");
        let mut task = spawn_gateway(options, listener, topology().await.expect("topology"));
        let client = Client::builder()
            .base_uri(&format!("http://{address}/sovd/v1"))
            .expect("URL")
            .build()
            .expect("client");
        let error = wait_for_ready(&mut task, &client).await.expect_err("invalid CORS");
        assert!(error.contains("credentials"), "{error}");
    }

    /// Runs the gateway on a free port of the loopback address with the extra `options`, such as
    /// `--upstream-url`, until the returned task is aborted.
    async fn serve(options: &[&str]) -> (tokio::task::JoinHandle<Result<(), String>>, Client) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let url = format!("http://{address}/sovd");
        let options = opensovd_gateway::cli::try_parse_from(
            ["opensovd-gateway", "--url", url.as_str()]
                .into_iter()
                .chain(options.iter().copied()),
        )
        .expect("options");
        let topology = topology().await.expect("topology");
        let mut task = spawn_gateway(options, listener, topology);
        let client = Client::builder()
            .base_uri(&format!("{url}/v1"))
            .expect("URL")
            .timeout(Duration::from_secs(5))
            .build()
            .expect("client");
        wait_for_ready(&mut task, &client).await.expect("gateway ready");
        (task, client)
    }

    // # verifies: comp_req__uds_data_bridge__read_handlers, comp_req__uds_data_bridge__utf8_data, comp_req__sovd_gateway__hosted_entities
    #[tokio::test(flavor = "current_thread")]
    async fn registered_application_is_discoverable_and_serves_data() {
        let (task, client) = serve(&[]).await;
        let apps = client.list_apps().send().await.expect("list apps");
        assert!(apps.items.iter().any(|app| app.id == "score-diagnostics"));
        let app = client.app("score-diagnostics");
        let location = app.located_on().await.expect("app location");
        assert!(location.is_some_and(|component| component.id == "score-demo"));
        let version = app
            .data("application.version")
            .read()
            .send()
            .await
            .expect("app version");
        assert_eq!(version.data["value"].as_str(), Some("1.0.0"));
        task.abort();
        let _ = task.await;
    }

    /// A fake CDA on a Unix domain socket: every request gets one component, with an href to the
    /// CDA's own API, as the CDA sends them.
    fn serve_socket_upstream(path: &std::path::Path) -> tokio::task::JoinHandle<()> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::UnixListener::bind(path).expect("socket");
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 1024];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut buffer).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => request.extend_from_slice(&buffer[..read]),
                        }
                    }
                    let body = r#"{"items":[{"id":"ecu1","name":"ecu1","href":"http://localhost/Vehicle/v15/components/ecu1"}]}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        })
    }

    // # verifies: comp_req__sovd_gateway__upstream_config
    #[tokio::test(flavor = "current_thread")]
    async fn upstream_entities_are_listed_over_a_unix_socket() {
        let dir = std::env::temp_dir().join(format!("score-gateway-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("directory");
        let path = dir.join("cda.sock");
        let _ = std::fs::remove_file(&path);
        let cda = serve_socket_upstream(&path);
        let url = format!("unix:{}:/vehicle/v15", path.display());
        let (task, client) = serve(&["--upstream-url", &url]).await;
        let components = client.list_components().send().await.expect("list components");
        let ecu = components.items.iter().find(|component| component.id == "ecu1");
        let ecu = ecu.unwrap_or_else(|| panic!("ecu1 not listed: {components:?}"));
        let href = &ecu.href.0;
        assert!(
            href.starts_with("http://127.0.0.1:") && href.ends_with("/sovd/v1/components/ecu1"),
            "href not rewritten to the gateway: {href}"
        );
        task.abort();
        cda.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    // # verifies: comp_req__sovd_gateway__upstream_unavailable
    #[tokio::test(flavor = "current_thread")]
    async fn unreachable_upstream_keeps_local_entities_available() {
        let unused = std::net::TcpListener::bind("127.0.0.1:0").expect("port");
        let url = format!("http://{}/vehicle/v15", unused.local_addr().expect("address"));
        drop(unused);
        let (task, client) = serve(&["--upstream-url", &url]).await;
        let apps = client.list_apps().send().await.expect("list apps");
        assert!(apps.items.iter().any(|app| app.id == "score-diagnostics"));
        let error = client
            .app("sovd2uds")
            .list_data()
            .send()
            .await
            .expect_err("forwarded request");
        assert!(
            matches!(&error, opensovd_client::Error::ApiError { status, .. } if status.as_u16() == 502),
            "{error:?}"
        );
        assert!(
            !format!("{error:?}").contains(&url),
            "upstream address leaked: {error:?}"
        );
        task.abort();
        let _ = task.await;
    }
}
