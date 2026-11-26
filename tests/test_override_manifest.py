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
"""Tests for the checked Git override export."""

import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from tools.override_manifest import manifest_from_module


class OverrideManifestTest(unittest.TestCase):
    def test_check_detects_stale_manifest(self) -> None:
        root = Path(__file__).resolve().parent.parent
        with tempfile.TemporaryDirectory() as temp_dir:
            workspace = Path(temp_dir)
            (workspace / "tools").mkdir()
            script = workspace / "tools" / "override_manifest.py"
            _ = shutil.copyfile(root / "tools" / "override_manifest.py", script)
            _ = shutil.copyfile(root / "MODULE.bazel", workspace / "MODULE.bazel")
            _ = subprocess.run([sys.executable, str(script)], check=True)
            _ = subprocess.run([sys.executable, str(script), "--check"], check=True)
            _ = (workspace / "git_overrides.json").write_text("{}\n")
            result = subprocess.run(
                [sys.executable, str(script), "--check"],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("stale", result.stderr)

    def test_extracts_all_attributes_and_sorts(self) -> None:
        a = "a" * 40
        b = "b" * 40
        source = f'''
git_override(module_name="z", remote="https://example/z", commit="{a}",
             patches=["//patches:fix.patch"], patch_strip=1)
git_override(module_name="a", remote="https://example/a", commit="{b}")
'''
        entries = manifest_from_module(source)["git_overrides"]
        self.assertEqual([entry["module_name"] for entry in entries], ["a", "z"])
        self.assertEqual(entries[1]["patches"], ["//patches:fix.patch"])
        self.assertEqual(entries[1]["patch_strip"], 1)

    def test_rejects_nonliteral_missing_or_unpinned_values(self) -> None:
        commit = "a" * 40
        for source in (
            'git_override(module_name="x", remote="url", commit=REV)',
            'git_override(module_name="x", remote="url", commit="main")',
            f'git_override(module_name="x", remote="url", commit="{commit}", patches=("x",))',
            f'git_override("x", remote="url", commit="{commit}")',
        ):
            with self.subTest(source=source), self.assertRaises(ValueError):
                _ = manifest_from_module(source)

    def test_duplicate_modules_rejected(self) -> None:
        commit = "a" * 40
        override = f'git_override(module_name="x", remote="url", commit="{commit}")'
        with self.assertRaises(ValueError):
            _ = manifest_from_module(override + "\n" + override)


if __name__ == "__main__":
    _ = unittest.main()
