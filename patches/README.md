<!-- SPDX-FileCopyrightText: 2026 Contributors to the Eclipse Foundation -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# QNX external crate patches

Diagnostics owns the six canonical external-crate payloads in this package.
They retain the reviewed upstream bytes, versions and license notices. In
particular, the AWS-LC patch changes compiler compatibility, not cryptographic
sources. The libc 0.2.191 patch is needed for Rust's `nto`/`nto80` cfgs even
when the standard toolchain supplies `QNX_HOST`.

`cda-qnx.patch` and `core-qnx.patch` are generated module overlays, not
independent copies to maintain. They replace the published modules' original
crate annotations and deliver the canonical payloads into a module-local
`score_qnx_patches` Bazel package. The CDA CAN universe receives the same libc
payload. Core keeps its isolated extension and all nine platform selectors.
A local extension identity lets its generated repositories resolve those
selectors with the selected consumer `rules_rust`. This does not apply
Core's standalone, root-only `0005` stock-rules metadata patch downstream.

Direct CDA and Core source fixes stay in their published owning repositories.
Their old external patch files remain present but unreferenced until upstream
cleanup. No crate gets both the old and diagnostics-owned annotation.

## Generation and export

`upstream_modules.json` stores the exact published MODULE inputs. The importer
reads immutable Git objects, never either upstream's dirty checkout:

```sh
python3 tools/qnx_overlays.py \
  --import-cda /path/to/classic-diagnostic-adapter \
  --import-core /path/to/opensovd-core
python3 tools/override_manifest.py
```

The source commits and expected input hashes are explicit in the generator.
After changing a canonical payload or input, review its provenance and update
the expected hash before regenerating. Do not edit nested patch copies:

```sh
python3 tools/qnx_overlays.py
python3 tools/qnx_overlays.py --check
python3 tools/override_manifest.py --check
python3 -m unittest discover -s tests -p test_override_manifest.py
python3 tests/bzlmod_patch_integration_test.py
python3 tests/bzlmod_consumer_test.py
```

Use a `TMPDIR` under the development volume for local fixture tests. The existing
schema 1 manifest exports the two flat `//patches:<name>` overlay labels with
`patch_strip = 1`. Downstream roots must repeat these overrides and copy the
exported patch package, as `tests/bzlmod_consumer_test.py` does. No root
annotation or cross-module include is needed to reach isolated crate universes.

## Cross-compilation configuration and validation status

This compatibility change configures the exact, unpatched standard
`score_bazel_cpp_toolchains` 1.1.0-rc3 provider, its default shared SDP 8.0.5
package, qcc 12.2.0 and the existing Ferrocene 1.94-nightly QNX toolchains.
It does not patch the provider or change the SDK environment.

The two application outputs are `//score/opensovd-core:opensovd-gateway` and
`//score/opensovd-cda:opensovd-cda`. No QNX application compilation or runtime
evidence has been recorded for this candidate; configuration and patch-delivery
checks do not establish that these outputs build or run.

PR 104 contains no new QNX workflow. A separate, unpublished CI change is planned
to build both application targets for x86_64 and AArch64. Maintainers must grant
the standard SCORE license/account/package secret references before running that
new CI. This prerequisite does not block consumption by an already licensed
SCORE integration root.

An integration root must register the exported overrides and use the generated
patch delivery described above. Do not maintain an independent root patch copy
or introduce the toolchain PR 206 overlay.
