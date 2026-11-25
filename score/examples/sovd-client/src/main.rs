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
use std::time::Duration;
use tower::util::MapRequestLayer;
use tracing_subscriber::EnvFilter;

const DEFAULT_URL: &str = "http://127.0.0.1:7690/sovd/v1";
// The placeholder authentication of the gateway accepts any non-empty credentials. The secret is the
// one that the default security plugin of the CDA accepts with its `auth` feature.
const CLIENT_ID: &str = "sovd-example-client";
const CLIENT_SECRET: &str = "secret";

fn connect(url: &str, token: Option<&str>) -> Result<Client, Box<dyn std::error::Error>> {
    let authorization = token
        .map(|token| {
            let mut value = http::HeaderValue::from_str(&format!("Bearer {token}"))?;
            value.set_sensitive(true);
            Ok::<_, http::header::InvalidHeaderValue>(value)
        })
        .transpose()?;
    let layer = MapRequestLayer::new(move |mut request: http::Request<_>| {
        if let Some(value) = &authorization {
            request.headers_mut().insert(http::header::AUTHORIZATION, value.clone());
        }
        request
    });
    Ok(Client::builder()
        .base_uri(url)?
        .timeout(Duration::from_secs(5))
        .layer(layer)
        .build()?)
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn is_unsupported_collection(error: &opensovd_client::Error) -> bool {
    matches!(error, opensovd_client::Error::ApiError { status, .. } if status.as_u16() == 404)
}

/// Gets the bearer token of the client from `authorize` of the gateway at `url`.
async fn fetch_token(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let response = connect(url, None)?.authorize(CLIENT_ID, CLIENT_SECRET).await?;
    Ok(response.access_token)
}

async fn run(client: &Client) -> Result<(), opensovd_client::Error> {
    let components = client.list_components().send().await?;
    for component in &components.items {
        tracing::info!("component: {} ({})", component.id, component.name);
    }

    if let Some(component) = components.items.first() {
        match client.component(&component.id).list_data().send().await {
            Ok(data) => {
                for item in &data.items {
                    tracing::info!("  data: {} ({})", item.id, item.name);
                }
            },
            Err(error) if is_unsupported_collection(&error) => {},
            Err(error) => return Err(error),
        }
    }

    let apps = client.list_apps().send().await?;
    for app in &apps.items {
        tracing::info!("app: {} ({})", app.id, app.name);
    }

    match client.list_areas().send().await {
        Ok(areas) => {
            for area in &areas.items {
                tracing::info!("area: {} ({})", area.id, area.name);
            }
        },
        Err(error) if is_unsupported_collection(&error) => {},
        Err(error) => return Err(error),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use opensovd_core::Component;
    use opensovd_server::{Server, Topology};
    use tokio::net::TcpListener;

    #[tokio::test(flavor = "current_thread")]
    async fn discovery_continues_without_component_data() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let topology = Topology::new();
        topology
            .write()
            .await
            .add_component(Component::new("no-data", "No Data"));
        let server = Server::builder()
            .base_uri(format!("http://{address}/sovd"))
            .expect("base URI")
            .listener(listener)
            .topology(topology)
            .build()
            .expect("server");
        let server_task = tokio::spawn(async move { server.serve().await });
        let client = connect(&format!("http://{address}/sovd/v1"), None).expect("client");
        let result = run(&client).await;
        server_task.abort();
        let _ = server_task.await;
        assert!(result.is_ok(), "unsupported data must not abort discovery: {result:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn configured_client_times_out_against_unresponsive_peer() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let peer = tokio::spawn(async move {
            let (_connection, _) = listener.accept().await.expect("accept connection");
            std::future::pending::<()>().await;
        });
        let client = connect(&format!("http://{address}/sovd/v1"), None).expect("client");
        let result = tokio::time::timeout(Duration::from_secs(6), run(&client)).await;
        peer.abort();
        let _ = peer.await;
        assert!(matches!(result, Ok(Err(opensovd_client::Error::Timeout(_)))));
    }

    #[test]
    fn only_not_found_marks_a_collection_unsupported() {
        let not_found = opensovd_client::Error::ApiError {
            status: "404".parse().expect("status"),
            details: None,
        };
        let server_error = opensovd_client::Error::ApiError {
            status: "500".parse().expect("status"),
            details: None,
        };
        let timeout = opensovd_client::Error::Timeout(std::time::Duration::from_secs(1));
        assert!(is_unsupported_collection(&not_found));
        assert!(!is_unsupported_collection(&server_error));
        assert!(!is_unsupported_collection(&timeout));
    }

    #[test]
    fn invalid_bearer_header_is_rejected() {
        assert!(connect("http://localhost/sovd/v1", Some("invalid\ntoken")).is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn configured_client_sends_bearer_token() {
        use opensovd_server::{Auth, AuthError, Authenticated, Authenticator, Parts, Principal};

        struct TestToken;
        impl Authenticator for TestToken {
            fn authenticate(&self, parts: &Parts) -> Result<Option<Principal>, AuthError> {
                if parts.headers.get(http::header::AUTHORIZATION).and_then(|v| v.to_str().ok())
                    != Some("Bearer test-token")
                {
                    return Ok(None);
                }
                Ok(Some(Principal {
                    subject: "test".into(),
                    roles: Vec::new(),
                    scopes: Vec::new(),
                    claims: Default::default(),
                }))
            }
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let server = Server::builder()
            .base_uri(format!("http://{address}/sovd"))
            .expect("base URI")
            .listener(listener)
            .auth(Auth::new(TestToken, Authenticated))
            .build()
            .expect("server");
        let task = tokio::spawn(async move { server.serve().await });
        let url = format!("http://{address}/sovd/v1");
        let unauthenticated = connect(&url, None).expect("client");
        assert!(matches!(
            unauthenticated.list_apps().send().await,
            Err(opensovd_client::Error::ApiError { status, .. }) if status.as_u16() == 401
        ));
        let authenticated = connect(&url, Some("test-token")).expect("client");
        assert!(authenticated.list_apps().send().await.is_ok());
        task.abort();
        let _ = task.await;
    }

    /// A server that answers every request with `status` and `body`, and sends back the request.
    async fn answer_once(status: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let url = format!("http://{}/sovd/v1", listener.local_addr().expect("address"));
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = vec![0u8; 4096];
            let read = stream.read(&mut request).await.expect("read request");
            request.truncate(read);
            stream.write_all(response.as_bytes()).await.expect("write response");
            String::from_utf8_lossy(&request).into_owned()
        });
        (url, task)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn token_comes_from_authorize() {
        let body = r#"{"access_token":"issued.jwt.token","token_type":"Bearer","expires_in":3600}"#;
        let (url, server) = answer_once("200 OK", body).await;
        let token = fetch_token(&url).await.expect("token");
        assert_eq!(token, "issued.jwt.token");
        let request = server.await.expect("server");
        assert!(request.starts_with("POST /sovd/v1/authorize "), "{request}");
        assert!(request.contains(&format!(r#""client_id":"{CLIENT_ID}""#)), "{request}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn authorize_refused_is_an_error() {
        let (url, server) = answer_once("401 Unauthorized", "{}").await;
        assert!(fetch_token(&url).await.is_err());
        let _ = server.await;
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let url = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("SOVD_BASE_URL").ok())
        .unwrap_or_else(|| DEFAULT_URL.to_owned());
    // The client's bearer credential: a JWT with `sub` and `exp` claims. The gateway has no token
    // setting; it checks the token's shape (placeholder) or verifies it against its JWKS.
    let token = fetch_token(&url).await?;
    let client = connect(&url, Some(&token))?;
    run(&client).await?;
    Ok(())
}
