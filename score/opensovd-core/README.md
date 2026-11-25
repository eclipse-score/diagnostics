<!--
*******************************************************************************
Copyright (c) 2026 Contributors to the Eclipse Foundation

See the NOTICE file(s) distributed with this work for additional
information regarding copyright ownership.

This program and the accompanying materials are made available under the
terms of the Apache License Version 2.0 which is available at
https://www.apache.org/licenses/LICENSE-2.0

SPDX-License-Identifier: Apache-2.0
*******************************************************************************
-->

# S-CORE OpenSOVD gateway

`//score/opensovd-core:opensovd-gateway` is the SOVD server of S-CORE (ISO 17978-3). It runs the
gateway of opensovd-core (`@opensovd_core//:opensovd-gateway-lib`) with the entities of S-CORE, so
it takes the options and `SOVD_*` environment variables of the reference gateway of opensovd-core
(`opensovd-gateway --help` there). Those that S-CORE uses:

| Variable | Option | Meaning | Default |
|---|---|---|---|
| `SOVD_URL` | `--url` | Address, port and base path to serve on | `http://localhost:7690/sovd` |
| `SOVD_UPSTREAM_URL` | `--upstream-url` | The CDA to forward what the gateway does not host to: `http://<host>:<port>[<base path>]`, e.g. `http://127.0.0.1:20002/vehicle/v15`, or a Unix domain socket `unix:<socket>[:<base path>]`, e.g. `unix:///run/cda.sock:/vehicle/v15` | no forwarding |
| `SOVD_AUTH_JWKS` | `--auth-jwks` | File containing the issuer's public verification keys in JWKS format | unset: placeholder authentication |
| `SOVD_AUTH_ISSUER` | `--auth-issuer` | Expected issuer of the client tokens, required with JWKS | unset |
| `SOVD_MDNS` | `--mdns` | Announce the gateway via mDNS/DNS-SD with this vehicle identification, e.g. the VIN | no announcement |
| `SOVD_MDNS_HOST` | `--mdns-host` | Host label of the announcement, published as `<host>.local` | derived from the identification |
| `SOVD_MDNS_INTERFACE` | `--mdns-interface` | Comma separated interfaces to announce on, for an unspecified address | all non-loopback interfaces |

With an upstream, the federation of opensovd-core lists the entities of the CDA next to the
gateway's own and forwards to it, over TCP or a Unix domain socket. Authentication, TLS and CORS
are those of opensovd-core as well.

## Authentication

Every endpoint besides `version-info` and `authorize` needs a token (MBDS S-SOVD 2024-07: "Every
endpoint besides /version-info, /authorize and /token shall be secured via tokens"). A client
reads `version-info` to discover the server before it has a token (ISO 17978-3 §5.11), so the
gateway answers that without one, and it answers `POST {base}/v1/authorize` without one, which is
where a client gets its token (ISO 17978-3 §7.23.4). A request for anything else without a bearer
token is answered with 401, including a path that the gateway forwards to the CDA, so nothing
reaches the CDA without a token.

Without `--auth-jwks` the gateway uses a placeholder that mirrors the default security plugin of
the CDA: it accepts any JWT-shaped bearer token, and `authorize` issues such a token to any caller.
**The gateway does not verify signatures in this mode and does not check the credentials of
`authorize`**, so anyone who can reach the gateway can authenticate. The placeholder only exists
until S-CORE has an identity and an issuer. The CDA parses the forwarded token as a JWT with a
`sub` and an `exp` claim, which is why the gateway requires that shape and issues tokens with these
claims. The authorization header is forwarded to the CDA, so a token from the gateway's `authorize`
works at the gateway and at the CDA.

For production, configure `--auth-jwks` and `--auth-issuer`. The JWKS contains the external issuer's
public keys only; the gateway verifies client tokens with them and never holds a signing key.
Clients obtain a JWT from the external issuer and send it as `Authorization: Bearer <token>`. With
`--auth-jwks` the gateway serves no `authorize`, and `version-info` needs a token unless a policy
(`--auth-policy`) allows it, as the authentication of opensovd-core does.

```sh
opensovd-gateway --auth-jwks /etc/sovd/issuer-jwks.json --auth-issuer https://issuer.example
```

The example client gets its token from `authorize` of the gateway. It sends fixed credentials, which
the placeholder accepts and which the CDA accepts with its `auth` feature, so it does not work against
a gateway that verifies tokens of an external issuer: that gateway serves no `authorize`.

### Getting a token

The gateway serves `POST /sovd/v1/authorize` itself, like the CDA's `/vehicle/v15/authorize`; the
CDA's is not reachable through the gateway. The body is JSON with `client_id` and `client_secret`.
Both have to be non-empty strings, otherwise the answer is 400, as it is for a body that is not
that JSON object. The placeholder does not check them: the `client_id` becomes the `sub` claim.

```sh
TOKEN=$(curl --fail -X POST http://127.0.0.1:7690/sovd/v1/authorize \
  -d '{"client_id":"tester","client_secret":"secret"}' | jq -r .access_token)

curl --fail -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7690/sovd/v1/apps
```

The answer is `{"access_token": "<jwt>", "token_type": "Bearer", "expires_in": 3600}`. The token is
valid for `expires_in` seconds, as its `exp` claim says. It is signed with HS256 and the key
`secret`, the development key of the CDA's default security plugin, so it also works with a CDA
that verifies tokens (built with its `auth` feature). That key is a development placeholder: it is
public, anyone can forge a token with it, and the gateway itself does not verify signatures in
placeholder mode. S-CORE does not provide an issuer yet.

## mDNS/DNS-SD

With `SOVD_MDNS` set, the gateway announces itself as a `_sovd._tcp` service (ISO 17978-3 §5.11),
so that a client on the same network finds it (ISO 17978-2 UC01 "Discover SOVD server(s)"). The
service has the TXT records `identification` (the value of `SOVD_MDNS`) and `accessurl` (the
`SOVD_URL` with the address of the gateway, the parent of `version-info`).

- The gateway has to listen on an address that clients can reach: `0.0.0.0` announces all
  non-loopback addresses, an interface address only that interface. A loopback address is refused
  at startup.
- The announcement starts once the server is set up and follows interface and address changes.
  On SIGTERM (the launch manager stops the gateway with it) or Ctrl-C the gateway sends a goodbye,
  so clients drop it at once.

Find it from another machine on the network:

```sh
dns-sd -B _sovd._tcp                 # macOS
avahi-browse -rt _sovd._tcp          # Linux
```
