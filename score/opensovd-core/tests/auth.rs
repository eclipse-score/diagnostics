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
        ])
        .expect("options");
        let auth = auth::configured(&options, Some("test-token".into()))
            .expect("configuration")
            .expect("static auth");
        drop(reservation);
        let task = TaskGuard(tokio::spawn(opensovd_gateway::run_with_auth_and_shutdown(
            options,
            Topology::default(),
            auth,
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

    let accepted = request(address, Some("Bearer test-token")).await;
    assert!(accepted.starts_with("HTTP/1.1 200"), "{accepted}");
    assert!(accepted.ends_with("ok"), "{accepted}");
    let requests = requests.lock().expect("requests");
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].starts_with("GET /vehicle/v15/functions "),
        "{}",
        requests[0]
    );
}

#[test]
fn missing_or_empty_configuration_is_rejected() {
    let options = opensovd_gateway::cli::try_parse_from(["gateway"]).expect("options");
    assert!(auth::configured(&options, None).is_err());
    assert!(auth::configured(&options, Some(String::new())).is_err());
    assert!(auth::configured(&options, Some(" ".into())).is_err());
    assert!(auth::configured(&options, Some("a=b".into())).is_err());
    assert!(auth::configured(&options, Some("==".into())).is_err());
}
