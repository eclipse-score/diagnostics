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

use axum::Json;
use axum::body::Bytes;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use opensovd_models::{ErrorCode, GenericError};
use opensovd_server::{
    Access, Auth, AuthError, Authenticator, Authorizer, MethodRouter, Parts, Principal,
};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

/// How long a token issued by [`authorize`] is valid, in seconds.
const TOKEN_LIFETIME: u64 = 3600;

/// Placeholder authenticator that requires a bearer token but does not verify it.
///
/// It mirrors the default security plugin of the CDA (without its `auth` feature): a missing token
/// is answered with 401, any well-formed token is accepted and its signature is NOT checked. The
/// token must be JWT-shaped (three dot-separated base64url segments), because the CDA parses the
/// forwarded token as a JWT with a `sub` and an `exp` claim.
///
/// This is a placeholder until S-CORE has an identity. Anyone who can reach the gateway can
/// authenticate with a forged token. Replace it with public-key verification by starting the
/// gateway with `--auth-jwks` and `--auth-issuer`.
pub struct AnyBearerToken;

impl Authenticator for AnyBearerToken {
    fn authenticate(&self, parts: &Parts) -> Result<Option<Principal>, AuthError> {
        let Some(header) = parts.headers.get("authorization") else {
            return Ok(None);
        };
        let header = header
            .to_str()
            .map_err(|_| AuthError::InvalidToken("invalid header".into()))?;
        let (scheme, token) = header.split_once(' ').unwrap_or((header, ""));
        if !scheme.eq_ignore_ascii_case("bearer") {
            return Err(AuthError::InvalidToken("unsupported scheme".into()));
        }
        if !jwt_shaped(token.trim_start()) {
            return Err(AuthError::InvalidToken("not a JWT".into()));
        }
        Ok(Some(Principal {
            subject: "unverified".into(),
            roles: Vec::new(),
            scopes: Vec::new(),
            claims: Default::default(),
        }))
    }
}

/// Three nonempty dot-separated base64url segments.
fn jwt_shaped(token: &str) -> bool {
    let mut segments = 0;
    for segment in token.split('.') {
        segments += 1;
        if segment.is_empty()
            || !segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return false;
        }
    }
    segments == 3
}

/// Requires a principal, except for the two resources that exist before a client has a token.
///
/// Every endpoint besides `version-info`, `authorize` and `token` is secured via tokens
/// (MBDS S-SOVD 2024-07, "Every endpoint besides /version-info, /authorize and /token shall be
/// secured via tokens"). A client discovers the server with `version-info` before it can have a
/// token (ISO 17978-3 §5.11), and gets its token from `authorize`. The gateway issues tokens at
/// `authorize` only, so `token` stays secured like the rest, and answers `resource-not-found`.
///
/// The routes are those of `Access::route`, the template of the matched route relative to the
/// mount path. A forwarded request has the route of the gateway that it matched: the routes of
/// entities and their resources, which need a principal. A path without a route of the gateway
/// has none, and needs a principal too, so nothing reaches the CDA without a token.
pub struct TokenRequired;

impl Authorizer for TokenRequired {
    fn authorize(&self, access: &Access<'_>) -> bool {
        access.principal.is_some()
            || matches!(
                (access.method, access.route),
                ("GET", Some("/version-info")) | ("POST", Some("/v1/authorize"))
            )
    }
}

/// The HS256 key that tokens from [`authorize`] are signed with: the development key of the CDA's
/// default security plugin (`KEYS` of cda-plugin-security).
///
/// PLACEHOLDER until S-CORE has an identity and an issuer. It is public, so anyone can forge a token
/// with it. It makes the token valid for a CDA that verifies tokens (built with its `auth` feature)
/// as well. The gateway's placeholder authenticator does NOT verify signatures.
const CDA_DEVELOPMENT_KEY: &[u8] = b"secret";

/// A token for `subject` that expires `TOKEN_LIFETIME` seconds after `now`, signed with HS256 and
/// [`CDA_DEVELOPMENT_KEY`]. The claims are the CDA's `Claims`: a string `sub` and a numeric `exp`.
fn placeholder_token(subject: &str, now: u64) -> Result<String, jsonwebtoken::errors::Error> {
    let claims = json!({ "sub": subject, "exp": now + TOKEN_LIFETIME });
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(CDA_DEVELOPMENT_KEY)?,
    )
}

fn error(status: StatusCode, error_code: ErrorCode, vendor_code: Option<&str>, message: &str) -> Response {
    let body = GenericError {
        error_code,
        vendor_code: vendor_code.map(str::to_owned),
        message: message.to_owned(),
        translation_id: None,
        parameters: None,
    };
    (status, Json(body)).into_response()
}

/// `POST /authorize`: issues a placeholder token (ISO 17978-3 §7.23.4, MBDS S-SOVD 2024-07).
///
/// It takes and answers what the CDA's `/authorize` does, so a client uses either:
/// `{"client_id": "...", "client_secret": "..."}` is answered with
/// `{"access_token": "<jwt>", "token_type": "Bearer", "expires_in": <seconds>}`.
/// The credentials are not checked, as in the default security plugin of the CDA without its
/// `auth` feature: anyone who can reach the gateway gets a token. A body that is not that JSON
/// object is a 400, as at the CDA, and so are empty credentials (ISO 17978-3 Table 340).
/// The token has the `client_id` as `sub` and an `exp` claim, which is what the CDA reads from the
/// token the gateway forwards. It is signed with the CDA's development key, so a CDA that verifies
/// tokens accepts it too; see [`CDA_DEVELOPMENT_KEY`].
async fn authorize(body: Bytes) -> Response {
    let (client_id, client_secret) = match serde_json::from_slice::<Value>(&body) {
        Ok(Value::Object(request)) => match (
            request.get("client_id").and_then(Value::as_str),
            request.get("client_secret").and_then(Value::as_str),
        ) {
            (Some(client_id), Some(client_secret)) => (client_id.to_owned(), client_secret.to_owned()),
            _ => {
                return error(
                    StatusCode::BAD_REQUEST,
                    ErrorCode::VendorSpecific,
                    Some("bad-request"),
                    "client_id and client_secret are required strings",
                );
            },
        },
        _ => {
            return error(
                StatusCode::BAD_REQUEST,
                ErrorCode::VendorSpecific,
                Some("bad-request"),
                "the request body must be a JSON object with client_id and client_secret",
            );
        },
    };
    if client_id.is_empty() || client_secret.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            ErrorCode::IncompleteRequest,
            None,
            "client_id and client_secret must not be empty",
        );
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let Ok(access_token) = placeholder_token(&client_id, now) else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::SovdServerFailure,
            None,
            "Internal server error",
        );
    };
    let body = json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "expires_in": TOKEN_LIFETIME,
    });
    (
        StatusCode::OK,
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (header::PRAGMA, HeaderValue::from_static("no-cache")),
        ],
        Json(body),
    )
        .into_response()
}

/// `Some` supplies the placeholder authentication and the `authorize` endpoint that issues its
/// tokens; `None` selects Core's own JWT configuration (`--auth-jwks`, `--auth-issuer`), where an
/// external issuer issues the tokens and the gateway serves no `authorize`.
pub fn configured(options: &opensovd_gateway::cli::Cli) -> Option<Auth> {
    (!options.server.auth_enabled()).then(|| {
        Auth::new(AnyBearerToken, TokenRequired).with_authorize(MethodRouter::new().post(authorize))
    })
}
