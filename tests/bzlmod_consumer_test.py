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

import ast
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OVERRIDES = {"classic-diagnostic-adapter", "mbedtls-rs", "opensovd_core"}


def main() -> None:
    source = (ROOT / "MODULE.bazel").read_text(encoding="utf-8")
    overrides: dict[str, str] = {}
    for node in ast.parse(source).body:
        if not isinstance(node, ast.Expr) or not isinstance(node.value, ast.Call):
            continue
        call = node.value
        if not isinstance(call.func, ast.Name) or call.func.id != "git_override":
            continue
        for keyword in call.keywords:
            if keyword.arg == "module_name" and isinstance(keyword.value, ast.Constant):
                name = keyword.value.value
                segment = ast.get_source_segment(source, node)
                if isinstance(name, str) and segment is not None:
                    overrides[name] = segment

    if not OVERRIDES <= overrides.keys():
        raise ValueError(
            f"Missing upstream git overrides: {OVERRIDES - overrides.keys()}"
        )

    with tempfile.TemporaryDirectory(prefix="diagnostics-consumer-") as temp_dir:
        workspace = Path(temp_dir)
        shutil.copytree(ROOT / "patches", workspace / "patches")
        shutil.copyfile(ROOT / ".bazelversion", workspace / ".bazelversion")
        (workspace / "MODULE.bazel").write_text(
            'module(name = "diagnostics_consumer_test")\n'
            'bazel_dep(name = "score_diagnostics", version = "0.0.0")\n'
            f"local_path_override(module_name = 'score_diagnostics', path = {str(ROOT)!r})\n"
            + "\n".join(overrides[name] for name in sorted(OVERRIDES)),
            encoding="utf-8",
        )
        subprocess.run(
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
                    "@score_diagnostics//score/opensovd-cda:cda_test_databases_files)"
                ),
            ],
            cwd=workspace,
            check=True,
        )


if __name__ == "__main__":
    main()
