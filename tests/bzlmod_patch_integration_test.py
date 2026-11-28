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
"""Exercise a manifest-generated Git override and patch with Bazel offline."""

import os
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from tests.bzlmod_consumer_test import prepare_consumer_workspace
from tools.override_manifest import manifest_from_module
from tools.qnx_overlays import diff_file

ROOT = Path(__file__).resolve().parent.parent


def run_git(root: Path, env: dict[str, str], *args: str) -> str:
    """Run Git only in the disposable fixture, without user configuration."""
    result = subprocess.run(
        ["git", *args], cwd=root, env=env, check=True, capture_output=True, text=True
    )
    return result.stdout.strip()


def main() -> None:
    """Verify Bazel sees a target added by the copied patch and rejects bad strip."""
    with tempfile.TemporaryDirectory(prefix="diagnostics-patch-fixture-") as temp_dir:
        base = Path(temp_dir)
        git_repo = base / "upstream"
        git_repo.mkdir()
        env = os.environ.copy()
        env.update(
            {
                "GIT_MASTER": "1",
                "GIT_CONFIG_GLOBAL": os.devnull,
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_CONFIG_COUNT": "0",
                "GIT_TERMINAL_PROMPT": "0",
                "GIT_ALLOW_PROTOCOL": "file",
            }
        )
        _ = run_git(git_repo, env, "init", "--quiet")
        _ = run_git(git_repo, env, "config", "user.name", "Manifest Fixture")
        _ = run_git(git_repo, env, "config", "user.email", "fixture@example.invalid")
        _ = (git_repo / "MODULE.bazel").write_text('module(name = "fixture_module")\n')
        _ = (git_repo / "BUILD.bazel").write_text('exports_files(["marker.txt"])\n')
        _ = (git_repo / "marker.txt").write_text("fixture\n")
        _ = run_git(git_repo, env, "add", "MODULE.bazel", "BUILD.bazel", "marker.txt")
        _ = run_git(git_repo, env, "commit", "--quiet", "-m", "Create fixture")
        commit = run_git(git_repo, env, "rev-parse", "HEAD")

        source = base / "diagnostics"
        (source / "patches").mkdir(parents=True)
        _ = (source / ".bazelversion").write_text((ROOT / ".bazelversion").read_text())
        _ = (source / "MODULE.bazel").write_text(
            'module(name = "score_diagnostics", version = "0.0.0")\n'
            + 'bazel_dep(name = "fixture_module", version = "0.0.0")\n'
        )
        _ = (source / "BUILD.bazel").write_text(
            'alias(name = "fixture_alias", actual = "@fixture_module//:patched_target", '
            + 'visibility = ["//visibility:public"])\n'
        )
        _ = (source / "patches" / "BUILD.bazel").write_text(
            'exports_files(["fixture.patch"])\n'
        )
        _ = (source / "patches" / "fixture.patch").write_text(
            "--- a/BUILD.bazel\n+++ b/BUILD.bazel\n@@ -1 +1,2 @@\n"
            + ' exports_files(["marker.txt"])\n'
            + '+filegroup(name = "patched_target", srcs = ["marker.txt", '
            + '"//nested_patches:payload.patch"], '
            + 'visibility = ["//visibility:public"])\n'
            + diff_file(
                "nested_patches/BUILD.bazel",
                "",
                'exports_files(["payload.patch"])\n',
            )
            + diff_file("nested_patches/payload.patch", "", "canonical payload\n")
        )
        registry = base / "empty-registry"
        registry.mkdir()
        # Bazel's embedded bazel_tools MODULE declares these modules even for
        # a query. Local empty modules keep the fixture independent of BCR.
        builtin_names = (
            "bazel_features",
            "buildozer",
            "platforms",
            "protobuf",
            "rules_cc",
            "rules_java",
            "rules_license",
            "rules_proto",
            "rules_python",
            "rules_shell",
            "zlib",
        )
        overrides: list[str] = []
        for name in builtin_names:
            builtin = base / "builtin" / name
            builtin.mkdir(parents=True)
            _ = (builtin / "MODULE.bazel").write_text(f'module(name = "{name}")\n')
            overrides.append(
                f'local_path_override(module_name = "{name}", path = {str(builtin)!r})'
            )
        for strip in (1, 0):
            workspace = base / f"consumer-{strip}"
            workspace.mkdir()
            module = (
                'git_override(module_name="fixture_module", '
                + f"remote={git_repo.as_uri()!r}, commit={commit!r}, "
                + f'patches=["//patches:fixture.patch"], patch_strip={strip})'
            )
            manifest = manifest_from_module(module)
            prepare_consumer_workspace(manifest, source, workspace)
            with (workspace / "MODULE.bazel").open("a", encoding="utf-8") as file:
                _ = file.write("\n" + "\n".join(overrides) + "\n")
            result = subprocess.run(
                [
                    "bazel",
                    "--batch",
                    "--ignore_all_rc_files",
                    f"--output_user_root={base / 'bazel'}",
                    "query",
                    f"--registry={registry.as_uri()}",
                    "--repository_disable_download",
                    "--incompatible_autoload_externally=",
                    "--lockfile_mode=off",
                    "deps(@score_diagnostics//:fixture_alias)",
                ],
                cwd=workspace,
                env=env,
                check=False,
                capture_output=True,
                text=True,
            )
            if strip == 1:
                if (
                    result.returncode
                    or "//:patched_target" not in result.stdout
                    or "//:marker.txt" not in result.stdout
                    or "//nested_patches:payload.patch" not in result.stdout
                ):
                    raise RuntimeError(f"patched target not resolved:\n{result.stderr}")
            elif (
                result.returncode == 0
                or "Error applying patch" not in result.stderr
                or "Cannot find file to patch" not in result.stderr
            ):
                raise RuntimeError(
                    f"wrong patch_strip was not rejected:\n{result.stderr}"
                )
            print(f"patch_strip={strip}: {'resolved' if strip == 1 else 'rejected'}")


if __name__ == "__main__":
    main()
