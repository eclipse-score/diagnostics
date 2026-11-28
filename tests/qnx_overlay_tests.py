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

"""Exercise canonical delivery through both independent module overlays."""

import ast
import hashlib
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from tests.bzlmod_consumer_test import Manifest, prepare_consumer_workspace
from tools import qnx_overlays
from tools.override_manifest import manifest_from_module
from tools.qnx_overlays import (
    PAYLOAD_HASHES,
    ROOT,
    SOURCES,
    TRIPLES,
    generated_overlays,
    load_modules,
    overlay_files,
)


class QnxOverlaysTest(unittest.TestCase):
    def assert_cargo_platforms(self, module: ast.Module) -> None:
        calls = [
            node
            for node in ast.walk(module)
            if isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and node.func.attr == "from_cargo"
        ]
        self.assertEqual(len(calls), 1)
        triples = [
            ast.literal_eval(keyword.value)
            for call in calls
            for keyword in call.keywords
            if keyword.arg == "supported_platform_triples"
        ]
        self.assertEqual(triples, [list(TRIPLES)])

    def test_generated_files_are_current_and_deterministic(self) -> None:
        generated = generated_overlays(ROOT / "patches")
        self.assertEqual(generated, generated_overlays(ROOT / "patches"))
        for name, expected in generated.items():
            self.assertEqual(
                (ROOT / "patches" / name).read_bytes(), expected.encode("ascii")
            )

    def test_generated_overlays_have_no_trailing_whitespace(self) -> None:
        for name, content in generated_overlays(ROOT / "patches").items():
            for number, line in enumerate(content.splitlines(), start=1):
                with self.subTest(name=name, line=number):
                    self.assertEqual(line, line.rstrip(" \t"))

    def test_payload_newline_changes_are_rejected(self) -> None:
        modules = load_modules(ROOT / "patches")
        with tempfile.TemporaryDirectory() as temp:
            patches = Path(temp)
            for payload in PAYLOAD_HASHES:
                _ = (patches / payload).write_bytes(
                    (ROOT / "patches" / payload).read_bytes()
                )
            payload = patches / "libc-0.2.191-qnx.patch"
            original = payload.read_bytes()
            self.assertEqual(
                hashlib.sha256(original).hexdigest(), PAYLOAD_HASHES[payload.name]
            )
            for name in SOURCES:
                _ = overlay_files(name, modules[name], patches)
            for newline in (b"\r\n", b"\r"):
                with self.subTest(newline=newline):
                    _ = payload.write_bytes(original.replace(b"\n", newline))
                    self.assertNotEqual(
                        hashlib.sha256(payload.read_bytes()).hexdigest(),
                        PAYLOAD_HASHES[payload.name],
                    )
                    for name in SOURCES:
                        with self.assertRaisesRegex(
                            ValueError, "unexpected source bytes"
                        ):
                            _ = overlay_files(name, modules[name], patches)

    def test_generated_overlay_newlines_are_checked_as_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            patches = base / "patches"
            patches.mkdir()
            for name in (*PAYLOAD_HASHES, "upstream_modules.json"):
                _ = (patches / name).write_bytes((ROOT / "patches" / name).read_bytes())
            expected = generated_overlays(patches)
            with patch.object(qnx_overlays, "ROOT", base):
                with patch("sys.argv", ["qnx_overlays.py"]):
                    qnx_overlays.main()
                with patch("sys.argv", ["qnx_overlays.py", "--check"]):
                    qnx_overlays.main()
                    for name, content in expected.items():
                        data = content.encode("ascii")
                        self.assertEqual((patches / name).read_bytes(), data)
                        for newline in (b"\r\n", b"\r"):
                            with self.subTest(name=name, newline=newline):
                                _ = (patches / name).write_bytes(
                                    data.replace(b"\n", newline)
                                )
                                with self.assertRaisesRegex(
                                    SystemExit, f"stale QNX overlays: {name}"
                                ):
                                    qnx_overlays.main()
                                _ = (patches / name).write_bytes(data)

    def test_cargo_platform_guard_rejects_missing_calls_or_triples(self) -> None:
        modules = load_modules(ROOT / "patches")
        for name in SOURCES:
            source = overlay_files(name, modules[name], ROOT / "patches")[
                "MODULE.bazel"
            ]
            self.assert_cargo_platforms(ast.parse(source))
            for invalid in (
                source.replace(".from_cargo(", ".removed_from_cargo("),
                source.replace(
                    "supported_platform_triples =", "removed_platform_triples ="
                ),
                source.replace(TRIPLES[0], "removed-platform"),
                source + "\ncrate.from_cargo(supported_platform_triples = [])\n",
            ):
                with (
                    self.subTest(name=name, source=invalid),
                    self.assertRaises(AssertionError),
                ):
                    self.assert_cargo_platforms(ast.parse(invalid))

    def test_exported_overlays_apply_and_preserve_isolated_annotations(self) -> None:
        manifest = manifest_from_module((ROOT / "MODULE.bazel").read_text())
        modules = load_modules(ROOT / "patches")
        with tempfile.TemporaryDirectory() as temp:
            base = Path(temp)
            consumer = base / "consumer"
            consumer.mkdir()
            prepare_consumer_workspace(manifest, ROOT, consumer)
            for name, module_name in (
                ("cda", "classic-diagnostic-adapter"),
                ("core", "opensovd_core"),
            ):
                entry = next(
                    entry
                    for entry in manifest["git_overrides"]
                    if entry["module_name"] == module_name
                )
                self.assertEqual(entry["commit"], SOURCES[name]["commit"])
                self.assertEqual(entry["patches"], [f"//patches:{name}-qnx.patch"])
                self.assertEqual(entry["patch_strip"], 1)
                upstream = base / name
                upstream.mkdir()
                _ = (upstream / "MODULE.bazel").write_text(modules[name])
                result = subprocess.run(
                    [
                        "patch",
                        "--batch",
                        "-p1",
                        "-i",
                        str(consumer / "patches" / f"{name}-qnx.patch"),
                    ],
                    cwd=upstream,
                    capture_output=True,
                    text=True,
                    check=True,
                )
                self.assertNotIn("fuzz", result.stdout)
                self.assertNotIn("offset", result.stdout)
                expected = overlay_files(name, modules[name], ROOT / "patches")
                for path, content in expected.items():
                    self.assertEqual((upstream / path).read_text(), content)
                module = ast.parse((upstream / "MODULE.bazel").read_text())
                self.assert_cargo_platforms(module)
                registrations: list[str] = []
                for node in ast.walk(module):
                    if not isinstance(node, ast.Call) or not isinstance(
                        node.func, ast.Attribute
                    ):
                        continue
                    if node.func.attr == "annotation":
                        for keyword in node.keywords:
                            if keyword.arg == "patches":
                                registrations.extend(ast.literal_eval(keyword.value))
                self.assertEqual(len(registrations), 4)
                for label in registrations:
                    self.assertTrue(label.startswith("//score_qnx_patches:"))
                    payload = label.split(":")[1]
                    data = (upstream / "score_qnx_patches" / payload).read_bytes()
                    self.assertEqual(
                        hashlib.sha256(data).hexdigest(), PAYLOAD_HASHES[payload]
                    )
                if name == "core":
                    self.assertIn("isolate = True", expected["MODULE.bazel"])
                    self.assertIn(
                        'platforms_template = "@opensovd_core//score_qnx_patches:{triple}"',
                        expected["MODULE.bazel"],
                    )
                    for triple in TRIPLES:
                        self.assertIn(triple, expected["score_qnx_patches/BUILD.bazel"])

    def test_tampered_payload_or_source_is_rejected(self) -> None:
        modules = load_modules(ROOT / "patches")
        for name in SOURCES:
            with self.assertRaisesRegex(ValueError, "unexpected source bytes"):
                _ = overlay_files(name, modules[name] + "\n", ROOT / "patches")
        with tempfile.TemporaryDirectory() as temp:
            patches = Path(temp)
            for name in PAYLOAD_HASHES:
                _ = (patches / name).write_bytes((ROOT / "patches" / name).read_bytes())
            with (patches / "libc-0.2.191-qnx.patch").open("a") as file:
                _ = file.write("tampered\n")
            for name in SOURCES:
                with self.assertRaisesRegex(ValueError, "unexpected source bytes"):
                    _ = overlay_files(name, modules[name], patches)
            for invalid in ("[]", '{"cda": 1, "core": ""}', '{"other": ""}'):
                _ = (patches / "upstream_modules.json").write_text(invalid)
                with self.assertRaises((TypeError, ValueError)):
                    _ = load_modules(patches)

    def test_manifest_does_not_expose_an_sdk_provider_overlay(self) -> None:
        manifest: Manifest = manifest_from_module((ROOT / "MODULE.bazel").read_text())
        self.assertEqual(
            {entry["module_name"] for entry in manifest["git_overrides"]},
            {"classic-diagnostic-adapter", "opensovd_core", "mbedtls-rs"},
        )

    def test_qnx_configuration_uses_unpatched_standard_provider(
        self,
    ) -> None:
        module = ast.parse((ROOT / "MODULE.bazel").read_text())
        toolchains: list[dict[str, object]] = []
        provider = "score_bazel_cpp_toolchains"
        for node in ast.walk(module):
            if not isinstance(node, ast.Call):
                continue
            if isinstance(node.func, ast.Name) and node.func.id in (
                "bazel_dep",
                "single_version_override",
                "git_override",
                "archive_override",
                "local_path_override",
            ):
                values = {
                    item.arg: ast.literal_eval(item.value)
                    for item in node.keywords
                    if item.arg is not None
                }
                if provider in (values.get("name"), values.get("module_name")):
                    self.assertEqual(node.func.id, "bazel_dep")
                    self.assertEqual(values.get("version"), "1.1.0-rc3")
            if (
                isinstance(node.func, ast.Attribute)
                and isinstance(node.func.value, ast.Name)
                and node.func.value.id == "gcc"
                and node.func.attr == "toolchain"
            ):
                values = {
                    item.arg: ast.literal_eval(item.value)
                    for item in node.keywords
                    if item.arg is not None
                }
                if values.get("target_os") == "qnx":
                    self.assertEqual(values.get("sdp_version"), "8.0.5")
                    self.assertEqual(values.get("version"), "12.2.0")
                    self.assertIs(values.get("use_default_package"), True)
                    self.assertNotIn("sdp_to_link", values)
                    toolchains.append(values)
        self.assertEqual(
            {item["target_cpu"] for item in toolchains}, {"x86_64", "aarch64"}
        )


if __name__ == "__main__":
    _ = unittest.main()
