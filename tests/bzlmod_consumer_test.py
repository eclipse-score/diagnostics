# *******************************************************************************
# Copyright (c) 2026 Contributors to the Eclipse Foundation
#
# See the NOTICE file(s) distributed with this work for additional
# information regarding copyright ownership.
#
# This program and the accompanying materials are made available under the
# terms of the Apache License Version 2.0 which is available at
# https://www.apache.org/licenses/LICENSE-2.0
#
# SPDX-License-Identifier: Apache-2.0
# *******************************************************************************
"""Check that downstream modules can load the public packaging targets."""

import json
import re
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import TypedDict, cast

ROOT = Path(__file__).resolve().parent.parent
OVERRIDES = {"classic-diagnostic-adapter", "mbedtls-rs", "opensovd_core"}


class Manifest(TypedDict):
    """Manifest schema consumed by the downstream smoke test."""

    schema_version: int
    git_overrides: list[dict[str, object]]


def prepare_consumer_workspace(
    manifest: Manifest, source_root: Path, workspace: Path
) -> None:
    """Create a consumer using every override and any supported root-local patches."""
    if manifest["schema_version"] != 1:
        raise ValueError("unsupported override manifest version")
    overrides: dict[str, dict[str, object]] = {}
    for entry in manifest["git_overrides"]:
        name = entry["module_name"]
        if not isinstance(name, str) or name in overrides:
            raise ValueError(f"invalid or duplicate module name: {name!r}")
        overrides[name] = entry

    # Validate all labels before copying or writing anything. This smoke test
    # supports only simple names in the copied root-local patches package.
    has_patches = False
    for override in overrides.values():
        patches = override.get("patches", [])
        if not isinstance(patches, list):
            raise TypeError("patches must be a list")
        for label in cast(list[object], patches):
            if not isinstance(label, str) or not re.fullmatch(
                r"//patches:[A-Za-z0-9_][A-Za-z0-9_.+-]*", label
            ):
                raise ValueError(f"unsupported root patch label: {label}")
            patch_dir = source_root / "patches"
            patch = patch_dir / label.removeprefix("//patches:")
            if not patch.is_file() or patch.resolve().parent != patch_dir.resolve():
                raise ValueError(f"missing or external patch file: {label}")
            has_patches = True
    if has_patches and not (source_root / "patches" / "BUILD.bazel").is_file():
        raise ValueError("missing patches/BUILD.bazel")

    _ = shutil.copyfile(source_root / ".bazelversion", workspace / ".bazelversion")
    if has_patches:
        _ = shutil.copytree(source_root / "patches", workspace / "patches")
    _ = (workspace / "MODULE.bazel").write_text(
        "\n".join(
            [
                'module(name = "diagnostics_consumer_test")',
                'bazel_dep(name = "score_diagnostics", version = "0.0.0")',
                f"local_path_override(module_name = 'score_diagnostics', path = {str(source_root)!r})",
                *(
                    "git_override(\n"
                    + "".join(
                        f"    {key} = {value!r},\n" for key, value in entry.items()
                    )
                    + ")"
                    for entry in overrides.values()
                ),
            ]
        ),
        encoding="utf-8",
    )


def main() -> None:
    """Load the checked snapshot and query downstream targets."""
    manifest = cast(
        Manifest, json.loads((ROOT / "git_overrides.json").read_text(encoding="utf-8"))
    )
    names = {entry.get("module_name") for entry in manifest["git_overrides"]}
    if not OVERRIDES <= names:
        raise ValueError(f"Missing upstream git overrides: {OVERRIDES - names}")

    with tempfile.TemporaryDirectory(prefix="diagnostics-consumer-") as temp_dir:
        workspace = Path(temp_dir)
        prepare_consumer_workspace(manifest, ROOT, workspace)
        result = subprocess.run(
            [
                "bazel",
                "--batch",
                "query",
                "--registry=https://raw.githubusercontent.com/eclipse-score/bazel_registry/main/",
                "--registry=https://bcr.bazel.build",
                "--experimental_isolated_extension_usages",
                "--lockfile_mode=off",
                (
                    "set(@score_diagnostics//score/opensovd-core:gateway_files "
                    "@score_diagnostics//score/opensovd-cda:cda_files "
                    "@score_diagnostics//score/opensovd-cda:cda_test_databases_files) + "
                    "labels(srcs, @score_diagnostics//score/opensovd-cda:needs_json) + "
                    "labels(deps, @score_diagnostics//score/opensovd-core:opensovd-gateway)"
                ),
            ],
            cwd=workspace,
            check=True,
            capture_output=True,
            text=True,
        )
        print(result.stdout, end="")
        if not any(
            line.endswith("//:requirements_docs") for line in result.stdout.splitlines()
        ):
            raise ValueError("CDA requirements_docs missing from downstream needs_json")
        if not any(
            line.endswith("//:opensovd-gateway-lib")
            for line in result.stdout.splitlines()
        ):
            raise ValueError("OpenSOVD integration gateway library is unavailable")


if __name__ == "__main__":
    main()
