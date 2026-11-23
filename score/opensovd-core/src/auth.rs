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

use opensovd_server::{Auth, AuthError, Authenticated, Authenticator, Parts, Principal};

/// A deployment-provided token; Core's Auth middleware handles the HTTP responses.
struct StaticToken(String);

impl Authenticator for StaticToken {
    fn authenticate(&self, parts: &Parts) -> Result<Option<Principal>, AuthError> {
        let Some(header) = parts.headers.get("authorization") else {
            return Ok(None);
        };
        let header = header
            .to_str()
            .map_err(|_| AuthError::InvalidToken("invalid header".into()))?;
        let (scheme, token) = header.split_once(' ').unwrap_or((header, ""));
        if !scheme.eq_ignore_ascii_case("bearer") {
            return Ok(None);
        }
        if aws_lc_rs::constant_time::verify_slices_are_equal(
            token.trim_start().as_bytes(),
            self.0.as_bytes(),
        )
        .is_err()
        {
            return Err(AuthError::InvalidToken("invalid token".into()));
        }
        Ok(Some(Principal {
            subject: "static-token".into(),
            roles: Vec::new(),
            scopes: Vec::new(),
            claims: Default::default(),
        }))
    }
}

/// `None` selects the reference gateway's JWT/Rego configuration.
pub fn configured(
    options: &opensovd_gateway::cli::Cli,
    token: Option<String>,
) -> Result<Option<Auth>, &'static str> {
    match (token, options.auth.jwks.is_some()) {
        (Some(_), true) => Err("SOVD_AUTH_TOKEN cannot be combined with --auth-jwks"),
        (Some(token), false) if valid_bearer_token(&token) => {
            Ok(Some(Auth::new(StaticToken(token), Authenticated)))
        }
        (Some(_), false) => Err("SOVD_AUTH_TOKEN must be a nonempty RFC 6750 bearer token"),
        (None, true) => Ok(None),
        (None, false) => Err("configure SOVD_AUTH_TOKEN or --auth-jwks and --auth-issuer"),
    }
}

fn valid_bearer_token(token: &str) -> bool {
    let unpadded = token.trim_end_matches('=');
    !unpadded.is_empty()
        && unpadded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/".contains(&b))
}
