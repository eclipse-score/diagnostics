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

# C++ & Rust Bazel Template Repository

This repository serves as a **template** for setting up **C++ and Rust projects** using **Bazel**.
It provides a **standardized project structure**, ensuring best practices for:

- **Build configuration** with Bazel.
- **Testing** (unit and integration tests).
- **Documentation** setup.
- **CI/CD workflows**.
- **Development environment** configuration.

---

## 📂 Project Structure

| File/Folder                         | Description                                       |
| ----------------------------------- | ------------------------------------------------- |
| `README.md`                         | Short description & build instructions            |
| `src/`                              | Source files for the module                       |
| `tests/`                            | Unit tests (UT) and integration tests (IT)        |
| `examples/`                         | Example files used for guidance                   |
| `docs/`                             | Documentation (Doxygen for C++ / mdBook for Rust) |
| `.github/workflows/`                | CI/CD pipelines                                   |
| `.vscode/`                          | Recommended VS Code settings                      |
| `.bazelrc`, `MODULE.bazel`, `BUILD` | Bazel configuration & settings                    |
| `project_config.bzl`                | Project-specific metadata for Bazel macros        |
| `LICENSE.md`                        | Licensing information                             |
| `CONTRIBUTION.md`                   | Contribution guidelines                           |

---

## 🚀 Getting Started

### 1️⃣ Clone the Repository

```sh
git clone https://github.com/eclipse-score/YOUR_PROJECT.git
cd YOUR_PROJECT
```

### 2️⃣ Build the Examples of module

> DISCLAIMER: Depending what module implements, it's possible that different
> configuration flags needs to be set on command line.

To build all targets of the module the following command can be used:

```sh
bazel build //src/...
```

This command will instruct Bazel to build all targets that are under Bazel
package `src/`. The ideal solution is to provide single target that builds
artifacts, for example:

```sh
bazel build //src/<module_name>:release_artifacts
```

where `:release_artifacts` is filegroup target that collects all release
artifacts of the module.

> NOTE: This is just proposal, the final decision is on module maintainer how
> the module code needs to be built.

### 3️⃣ Run Tests

```sh
bazel test //tests/...
```

---

## 🛠 Tools & Linters

The template integrates **tools and linters** from **centralized repositories** to ensure consistency across projects.

- **C++:** `clang-tidy`, `cppcheck`, `Google Test`
- **Rust:** `clippy`, `rustfmt`, `Rust Unit Tests`
- **CI/CD:** GitHub Actions for automated builds and tests

### CodeQL Findings

Assess production alerts separately from tests and mocks. The selected CodeQL
checks provide automated findings for review; this C++ scan does not analyze Rust
or establish safety across the C ABI.

For reusable analysis configuration and tooling, see the communication project's
[static-analysis setup](https://github.com/eclipse-score/communication/tree/main/quality/static_analysis).

The following remaining patterns need review rather than blanket suppression.
These are triage rationales, not approved exceptions:

| Check | Scope and Rationale |
|-------|---------------------|
| Global namespace declarations | The bridge's opaque types and callback typedefs form a C-compatible interface. C++ namespaces cannot replace those C declarations without redesigning the interface. |
| Use of `reinterpret_cast` | The bridge lends the byte buffer to a callback without copying. A compile-time assertion requires `uint8_t` to be `unsigned char`, which may inspect object representations. The callback must not retain the pointer; Rust copies the bytes before returning. |
| Smart-pointer factory functions | The bridge adopts handles using custom deleters that call the C release functions. `make_unique` does not construct these custom-deleter pointer types or adopt existing C handles. |
| Hidden inherited virtual functions | The `Simple*` UDS adapters deliberately expose synchronous overloads and privately finalize the cancellation-aware overloads. The existing tests exercise dispatch through the full base interfaces; renaming the synchronous methods would change the implementer API. |
| Implicit boolean conversions | The validator precondition and future-result check are reported as conversions from `unknown`. Treat these as analysis limitations pending manual type review, not demonstrated non-boolean conditions. |

GoogleTest/GoogleMock expansions also produce alerts at test call sites, including
generated branches, boolean conversions, linkage and mock exception paths. Keep
those findings visible in the test report, but do not modify vendored framework
code or weaken production interfaces merely to reduce their count. Intentional
contract-test temporaries and the bridge's callback-context cast also require
test-specific review. Alert counts are not counts of confirmed defects.

---

## 📖 Documentation

- A **centralized docs structure** is planned.

---

## ⚙️ `project_config.bzl`

This file defines project-specific metadata used by Bazel macros, such as `dash_license_checker`.

### 📌 Purpose

It provides structured configuration that helps determine behavior such as:

- Source language type (used to determine license check file format)
- Safety level or other compliance info (e.g. ASIL level)

### 📄 Example Content

```python
PROJECT_CONFIG = {
    "asil_level": "QM",  # or "ASIL-A", "ASIL-B", etc.
    "source_code": ["cpp", "rust"]  # Languages used in the module
}
```

### 🔧 Use Case

When used with macros like `dash_license_checker`, it allows dynamic selection of file types
 (e.g., `cargo`, `requirements`) based on the languages declared in `source_code`.

---

## 🔧 Toolchain Architecture

The OpenSOVD gateway, CDA and client are development prototypes. The gateway
target is public, `//score/opensovd-core:gateway_files` and
`//score/opensovd-cda:cda_files` package the gateway and the CDA for an image,
and `opensovd_core`, `classic-diagnostic-adapter` and `mbedtls-rs` are regular
`bazel_dep`s, so another Bazel module can build them. The example client stays
visible only to the CDA package.

Bazel only honours `git_override` in the root module. A consumer of
`score_diagnostics` must repeat these overrides and enable
`--experimental_isolated_extension_usages` in its `.bazelrc`.
The machine-readable [git_overrides.json](git_overrides.json) contains the
complete keyword arguments for each override in `git_overrides`, with
`schema_version` 1. Consumers should reject unknown schema versions and handle
all attributes, including patches if present; patch labels are relative to the
root module and must be copied and translated by the consumer. A generator
that cannot safely transfer an attribute or patch must reject the manifest
rather than omit it. In particular, the reference integration generator currently
rejects patch-bearing manifests; diagnostics' smoke test only supports local
`//patches:<filename>` labels and copies that package into its consumer root.
`MODULE.bazel`
is authoritative: after changing its overrides run
`python3 tools/override_manifest.py` and commit the regenerated manifest.
`python3 tools/override_manifest.py --check` detects drift in CI.

The CDA pin includes the requirements export merged in
[CDA PR #763](https://github.com/eclipse-opensovd/classic-diagnostic-adapter/pull/763).

The OpenSOVD pin is an unmerged commit on `feat/score-integration`. It includes
the integration APIs and a declared build-stamp file input; it does **not**
include the separate [OpenSOVD PR #302](https://github.com/eclipse-opensovd/opensovd-core/pull/302).

Callers decide whether to supply verified gateway provenance. First inspect the
resolved repository (for example with
`bazel mod show_repo opensovd_core --output=streamed_jsonproto`), verify the
commit and absence of source modifications, and obtain its commit timestamp
from a verified clone of the same upstream repository. A public Bazel target
in the consuming root may provide a file with `OPENSOVD_COMMIT_SHA=<full SHA>`
and `SOURCE_DATE_EPOCH=<commit timestamp>` on separate lines. Pass it with
`--@opensovd_core//opensovd-cli/gateway:build_stamp=//package:stamp_target`.
These are declared file inputs, not automatically inherited shell variables.
For local, archive or patched overrides, only supply values if the exact sources
are independently verified; the build does not verify them.

Without a supplied stamp file, the current upstream implementation falls back
to Git and can report metadata from the wrong checkout. Do not treat the
unstamped result as verified source provenance. PR #302 proposes a separate
approach to that fallback and is not included in the pinned branch.

All Linux targets, including the OpenSOVD gateway, CDA and example client, use
the `score_diag_x86_64_linux` configuration. Host build tools and target code
use the same hermetic toolchains:

| Component | Toolchain | Version |
|-----------|-----------|---------|
| **C/C++** | GCC (hermetic) | 12.2.0 |
| **Rust** | Ferrocene | Pinned by `score_toolchains_rust` 0.10.1 |

`rules_foreign_cc` 0.17.0 uses pkgconf for native dependency builds with the
hermetic GCC toolchain. Auto-detected system C/C++ toolchains are disabled.
Using the Ferrocene toolchain does not itself qualify the application code.

QNX dependency triples are not enabled in the shared crate index: the pinned
`rules_rust` crate generator rejects `aarch64-unknown-nto-qnx800` during dependency
generation. QNX support requires compatible dependency-generation tooling and
separate validation; restoring those entries alone breaks Linux builds too.

Strict C/C++ warnings are enabled. Warnings are errors for this repository's
sources in both target and host builds; external dependencies and vendored C
compiled by Rust build scripts retain nonfatal warnings.

### Configuration

Toolchains are registered in [MODULE.bazel](MODULE.bazel), and platform and
warning settings are defined in [.bazelrc](.bazelrc).

```sh
bazel build --config=score_diag_x86_64_linux //examples/... //tests/... //score/...
bazel test --config=score_diag_x86_64_linux //examples/... //tests/... //score/...
```

### ECU simulator test data

`//score/opensovd-cda:cda_test_databases_files` packages all ECU databases (MDD) of the ECU
simulator of the classic diagnostic adapter (CDA). Some of its ECUs only work with the settings
that the integration tests of the CDA make for them (`ignore_protocol`, the logical gateway
addresses, com-params): `score/opensovd-cda/cda-ecu-sim.toml` has exactly those and nothing else.
A consumer appends it to its own CDA configuration, which sets the paths, address and port.

The `On PR` workflow also runs `score/opensovd-cda/tests/ecu_sim_ci.sh`. It starts the `ecu-sim`
service of the pinned CDA's `testcontainer/docker-compose.yml` and runs
`//score/opensovd-cda:simulator_test`, whose JUnit report reaches the documentation's
`test-reports`. The test seeds a fault through the simulator's control API, checks a live VIN,
faults with snapshot and extended data, clear, routine lifecycle and hard reset through the gateway,
and finally checks the diagnostic requests the simulator recorded and its final state, so an empty
run cannot pass and any failure is in the report. The Core client performs GET and PUT requests; a
test-only `curl` helper performs POST and DELETE, which the pinned client does not expose. It needs
Linux, Docker Compose, `curl` and `iproute2`. It is meant for a throwaway CI runner: like upstream,
the simulator runs privileged on the fixed network `172.42.0.0/16` and publishes its control port
8181 on the host. The regular Bazel tests remain bus-free: only the manual `simulator_test_binary`
target enables the `ecu_simulator` Rust feature.

Seeding through the simulator control API is fixture setup, not an SOVD operation.
The pinned CDA reports an execution as `completed` after each positive RequestResults response,
even while the TimeCircuits result still reports less than 100% progress; the test checks both.

### Patches

No local dependency patches are currently required. The pinned CDA revision
exports `requirements_docs` for `//score/opensovd-cda:needs_json`.
