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
"""Export root-only Bazel Git overrides without evaluating MODULE.bazel."""

import argparse
import ast
import json
import re
from pathlib import Path
from typing import TypedDict, cast

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "git_overrides.json"


class Manifest(TypedDict):
    schema_version: int
    git_overrides: list[dict[str, object]]


def manifest_from_module(source: str) -> Manifest:
    """Keep every literal attribute; reject overrides we cannot faithfully export."""
    overrides: list[tuple[str, dict[str, object]]] = []
    names: set[str] = set()
    for node in ast.walk(ast.parse(source)):
        if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Name):
            continue
        if node.func.id != "git_override":
            continue
        if node.args or any(keyword.arg is None for keyword in node.keywords):
            raise ValueError("git_override must use explicit keyword arguments")
        values: dict[str, object] = {}
        for keyword in node.keywords:
            key = keyword.arg
            if key is None:
                raise ValueError("git_override must use explicit keyword arguments")
            if key in values:
                raise ValueError(f"duplicate attribute: {keyword.arg}")
            try:
                values[key] = ast.literal_eval(keyword.value)
            except (ValueError, SyntaxError) as error:
                raise ValueError(
                    f"non-literal git_override attribute: {keyword.arg}"
                ) from error
        name, remote, commit = (
            values.get(key) for key in ("module_name", "remote", "commit")
        )
        if not isinstance(name, str) or not name or name in names:
            raise ValueError(f"missing or duplicate module_name: {name!r}")
        if not isinstance(remote, str) or not remote:
            raise ValueError(f"{name}: missing remote")
        if not isinstance(commit, str) or not re.fullmatch(r"[0-9a-f]{40}", commit):
            raise ValueError(f"{name}: commit must be a full lowercase Git SHA")

        # JSON must represent the values exactly; tuples and non-string keys must not be coerced.
        def json_value(value: object) -> bool:
            if value is None or isinstance(value, (str, bool, int)):
                return True
            if isinstance(value, list):
                return all(json_value(item) for item in cast(list[object], value))
            if isinstance(value, dict):
                return all(
                    isinstance(k, str) and json_value(v)
                    for k, v in cast(dict[object, object], value).items()
                )
            return False

        if not json_value(values):
            raise ValueError(
                f"{name}: attribute cannot be represented faithfully in JSON"
            )
        names.add(name)
        overrides.append((name, values))
    return {
        "schema_version": 1,
        "git_overrides": [
            values for _, values in sorted(overrides, key=lambda x: x[0])
        ],
    }


def main() -> None:
    class Args(argparse.Namespace):
        check: bool = False

    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument(
        "--check", action="store_true", help="fail if the manifest is stale"
    )
    args = parser.parse_args(namespace=Args())
    rendered = (
        json.dumps(
            manifest_from_module((ROOT / "MODULE.bazel").read_text()),
            indent=2,
            sort_keys=True,
        )
        + "\n"
    )
    if args.check:
        if not MANIFEST.exists() or MANIFEST.read_text() != rendered:
            raise SystemExit(
                "git_overrides.json is stale; run python3 tools/override_manifest.py"
            )
    else:
        _ = MANIFEST.write_text(rendered)


if __name__ == "__main__":
    main()
