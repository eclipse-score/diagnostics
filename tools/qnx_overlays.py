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
# SPDX-FileCopyrightText: 2026 Contributors to the Eclipse Foundation

"""Generate module-local QNX patch delivery for independent crate universes."""

import argparse
import difflib
import hashlib
import json
import os
import subprocess
from pathlib import Path
from typing import TypedDict, cast

ROOT = Path(__file__).resolve().parent.parent
HEADER = """# SPDX-FileCopyrightText: 2026 Contributors to the Eclipse Foundation
# SPDX-License-Identifier: Apache-2.0
"""


class Source(TypedDict):
    commit: str
    module_sha256: str
    patches: dict[str, str]


SOURCES: dict[str, Source] = {
    "cda": {
        "commit": "7272ed1bf85fce562f741ec958e504421defa3c1",
        "module_sha256": "018ac504ec6498bcb84f95cfad038193c54e5757cdffe70ba1c6fa1786c657ae",
        "patches": {
            "0001-libc-qnx800-target-cfg.patch": "libc-0.2.191-qnx.patch",
            "0002-memmap2-qnx-posix-advice.patch": "memmap2-qnx.patch",
            "0003-linkme-impl-qnx-elf.patch": "linkme-impl-qnx.patch",
        },
    },
    "core": {
        "commit": "f639b04b00a138043b13bbc1c604e16844b58b7e",
        "module_sha256": "166b003e27cf9b0b01c77dd09066d29967cc917d281a2add47e5a6c5408b6052",
        "patches": {
            "0001-aws-lc-sys-qnx-compiler.patch": "aws-lc-sys-qnx.patch",
            "0002-libc-qnx800-target-cfg.patch": "libc-0.2.191-qnx.patch",
            "0003-if-addrs-qnx-platforms.patch": "if-addrs-qnx.patch",
            "0004-socket-pktinfo-qnx-platforms.patch": "socket-pktinfo-qnx.patch",
        },
    },
}
PAYLOAD_HASHES = {
    "aws-lc-sys-qnx.patch": "1c80505760353ac109cdf397d1e8806a516e197741b3d89e1c3429feff11ed98",
    "if-addrs-qnx.patch": "45ab809cee6469feb2e11070451e409603abaaaec0532afa562a8413cbd9d783",
    "libc-0.2.191-qnx.patch": "5747208fffee68c63cb025a2c52de4c99c732e3fd27e2858750c934648b694ef",
    "linkme-impl-qnx.patch": "7f4b869197faffe29f3ccdab8fb460a40b668f94bb2297ac0dc7b8aece84d638",
    "memmap2-qnx.patch": "afcef5e61af77186b1c47efd69d053566d28c67ea9ed53f7ea8b70b9b95446b2",
    "socket-pktinfo-qnx.patch": "0c90d1ee07762bcf728d0bda297c0ebd45bbf77ebb9bf2bbbdccc1164a62858d",
}
TRIPLES = (
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "aarch64-unknown-nto-qnx710",
    "wasm32-unknown-unknown",
    "wasm32-wasip1",
    "x86_64-pc-nto-qnx710",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "x86_64-unknown-nixos-gnu",
)


def verify(data: str, digest: str, name: str) -> str:
    if hashlib.sha256(data.encode("ascii")).hexdigest() != digest:
        raise ValueError(f"unexpected source bytes: {name}")
    return data


def replace_once(source: str, old: str, new: str, count: int = 1) -> str:
    if source.count(old) != count:
        raise ValueError(f"unexpected module context: {old!r}")
    return source.replace(old, new)


def read_git(repo: Path, commit: str, path: str) -> str:
    return subprocess.check_output(
        ["git", "-C", str(repo), "show", f"{commit}:{path}"],
        env={**os.environ, "GIT_MASTER": "1"},
    ).decode("ascii")


def imported_files(cda: Path, core: Path) -> dict[str, str]:
    """Read immutable Git objects, never either upstream worktree's dirty files."""
    modules: dict[str, str] = {}
    files: dict[str, str] = {}
    for name, repo in (("cda", cda), ("core", core)):
        spec = SOURCES[name]

        modules[name] = verify(
            read_git(repo, spec["commit"], "MODULE.bazel"), spec["module_sha256"], name
        )
        for old, new in spec["patches"].items():
            data = verify(
                read_git(repo, spec["commit"], f"patches/{old}"),
                PAYLOAD_HASHES[new],
                new,
            )
            if new in files and files[new] != data:
                raise ValueError(f"conflicting canonical payload: {new}")
            files[new] = data
    files["upstream_modules.json"] = (
        json.dumps(modules, indent=2, sort_keys=True) + "\n"
    )
    return files


def diff_file(path: str, before: str, after: str) -> str:
    prefix = f"diff --git a/{path} b/{path}\n"
    if not before:
        prefix += "new file mode 100644\n"
    lines = difflib.unified_diff(
        before.splitlines(keepends=True),
        after.splitlines(keepends=True),
        fromfile=f"a/{path}" if before else "/dev/null",
        tofile=f"b/{path}",
    )
    # Blank no-op replacements preserve hunk counts without trailing whitespace.
    return prefix + "".join("-\n+\n" if line == " \n" else line for line in lines)


def overlay_files(name: str, baseline: str, patches: Path) -> dict[str, str]:
    """Replace existing registrations and deliver canonical bytes within the module."""
    spec = SOURCES[name]
    module = verify(baseline, spec["module_sha256"], name)
    files: dict[str, str] = {}
    for old, new in spec["patches"].items():
        module = replace_once(
            module,
            f"//:patches/{old}",
            f"//score_qnx_patches:{new}",
            count=2 if name == "cda" and new == "libc-0.2.191-qnx.patch" else 1,
        )
        files[f"score_qnx_patches/{new}"] = verify(
            (patches / new).read_bytes().decode("ascii"), PAYLOAD_HASHES[new], new
        )
    exports = sorted(spec["patches"].values())
    build = HEADER
    if name == "core":
        module = replace_once(
            module,
            'bazel_dep(name = "rules_rust", version = "0.70.0")',
            'bazel_dep(name = "bazel_skylib", version = "1.9.2")\n'
            'bazel_dep(name = "platforms", version = "1.1.0")\n'
            'bazel_dep(name = "rules_cc", version = "0.2.25")\n'
            'bazel_dep(name = "rules_rust", version = "0.70.0")',
        )
        # A local extension identity makes the generated repositories resolve
        # Core's selectors rather than the stock rules_rust platform package.
        module = replace_once(
            module,
            '    "@rules_rust//crate_universe:extensions.bzl",',
            '    "//score_qnx_patches:extension.bzl",',
        )
        anchor = "    isolate = True,\n)\n"
        module = replace_once(
            module,
            anchor,
            anchor + "opensovd_crate_universe.render_config(\n"
            '    platforms_template = "@opensovd_core//score_qnx_patches:{triple}",\n'
            ")\n",
        )
        files["score_qnx_patches/extension.bzl"] = (
            HEADER
            + '\n"""Core-local identity for isolated crate-universe selectors."""\n\n'
            + 'load("@rules_rust//crate_universe:extensions.bzl", _crate = "crate")\n\n'
            + "crate = _crate\n"
        )
        exports.append("extension.bzl")
        build += '\nload("@rules_rust//rust/platform:triple_mappings.bzl", "triple_to_constraint_set")\n'
    build += "\nexports_files(" + repr(exports) + ")\n"
    if name == "core":
        build += (
            "\n[\n    config_setting(\n        name = triple,\n"
            "        constraint_values = triple_to_constraint_set(triple),\n"
            '        visibility = ["//visibility:public"],\n'
            "    )\n    for triple in [\n"
            + "".join(f'        "{triple}",\n' for triple in TRIPLES)
            + "    ]\n]\n"
        )
    files["score_qnx_patches/BUILD.bazel"] = build
    return {"MODULE.bazel": module, **dict(sorted(files.items()))}


def load_modules(patches: Path) -> dict[str, str]:
    data: object = json.loads(
        (patches / "upstream_modules.json").read_text(encoding="ascii")
    )
    if not isinstance(data, dict):
        raise TypeError("upstream modules must be an object")
    modules = cast(dict[object, object], data)
    if set(modules) != set(SOURCES):
        raise ValueError("unexpected upstream module names")
    if any(not isinstance(value, str) for value in modules.values()):
        raise TypeError("upstream modules must contain strings")
    return cast(dict[str, str], modules)


def generated_overlays(patches: Path) -> dict[str, str]:
    modules = load_modules(patches)
    return {
        f"{name}-qnx.patch": HEADER
        + f"# Generated by tools/qnx_overlays.py from {SOURCES[name]['commit']}.\n"
        + "# Do not edit nested payload copies; regenerate from canonical patches.\n"
        + "".join(
            diff_file(path, modules[name] if path == "MODULE.bazel" else "", content)
            for path, content in overlay_files(name, modules[name], patches).items()
        )
        for name in SOURCES
    }


def main() -> None:
    class Args(argparse.Namespace):
        check: bool = False
        import_cda: Path | None = None
        import_core: Path | None = None

    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--check", action="store_true")
    _ = parser.add_argument("--import-cda", type=Path)
    _ = parser.add_argument("--import-core", type=Path)
    args = parser.parse_args(namespace=Args())
    patches = ROOT / "patches"
    if args.import_cda or args.import_core:
        if args.check or not (args.import_cda and args.import_core):
            parser.error(
                "import requires both source repositories and cannot use --check"
            )
        for name, content in imported_files(args.import_cda, args.import_core).items():
            _ = (patches / name).write_bytes(content.encode("ascii"))
    generated = generated_overlays(patches)
    if args.check:
        stale = [
            name
            for name, content in generated.items()
            if not (patches / name).is_file()
            or (patches / name).read_bytes() != content.encode("ascii")
        ]
        if stale:
            raise SystemExit("stale QNX overlays: " + ", ".join(stale))
    else:
        for name, content in generated.items():
            _ = (patches / name).write_bytes(content.encode("ascii"))


if __name__ == "__main__":
    main()
