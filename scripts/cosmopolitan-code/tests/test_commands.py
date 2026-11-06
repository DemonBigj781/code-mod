import errno
import hashlib
import io
import json
import shutil
import subprocess
from pathlib import Path
import sys
import tomllib
import unittest
from contextlib import redirect_stderr
from unittest.mock import patch


sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import commands
import test_artifact_contract


class CommandTests(unittest.TestCase):
    def test_tui_self_dependency_does_not_reenable_native_picker(self):
        manifest_path = Path(__file__).resolve().parents[3] / "code-rs/tui/Cargo.toml"
        with manifest_path.open("rb") as stream:
            manifest = tomllib.load(stream)
        dependency = manifest["dev-dependencies"]["code-tui"]
        self.assertFalse(dependency.get("default-features", True))
        self.assertIn("test-helpers", dependency["features"])

    def test_build_jobs_are_one_except_on_github_hosted_runners(self):
        script = (Path(__file__).resolve().parents[1] / "prepare-runtime.sh").read_text()
        start = script.index("build_jobs=1\n")
        end = script.index("export CARGO_PROFILE_RELEASE_LTO", start)
        # Exercise only the resource-policy block, never the build commands.
        policy = 'getconf() { printf "7"; }\n' + script[start:end]
        policy += '\nprintf "%s,%s,%s" "$CARGO_BUILD_JOBS" "$CMAKE_BUILD_PARALLEL_LEVEL" "$MAKEFLAGS"\n'
        for actions, runner, expected in [
            ("", "", "1"), ("true", "self-hosted", "1"),
            ("false", "github-hosted", "1"), ("true", "github-hosted", "7"),
        ]:
            with self.subTest(actions=actions, runner=runner):
                result = subprocess.run(["sh", "-c", policy], capture_output=True, text=True,
                                        check=True, env={"GITHUB_ACTIONS": actions, "RUNNER_ENVIRONMENT": runner})
                self.assertEqual(result.stdout, f"{expected},{expected},-j{expected}")

    def test_default_rust_ape_cache_is_isolated_by_source_pins(self):
        script = (Path(__file__).resolve().parents[1] / "prepare-runtime.sh").read_text()
        assignment = (
            'build_root=${COSMO_RUST_BUILD_ROOT:-'
            '"$repo_root/o/rust-ape-source-$cosmo_commit-$rust_ape_commit"}'
        )
        self.assertIn(assignment, script)
        self.assertLess(script.index("rust_ape_commit=$(read_pin rust_ape_commit)"), script.index(assignment))

    def test_ape_build_uses_a_current_lock_with_patched_runtime_dependencies(self):
        support = Path(__file__).resolve().parents[1]
        script = (support.parent / "build-cosmopolitan-code.sh").read_text()
        lock = tomllib.loads((support / "Cargo.lock").read_text())
        packages = {package["name"]: package for package in lock["package"]}
        self.assertIn('cp "$support_dir/Cargo.lock"', script)
        self.assertIn("--locked", script)
        self.assertEqual(packages["libc"]["version"], "0.2.189")
        self.assertNotIn("source", packages["libc"])
        self.assertEqual(packages["errno"]["version"], "0.3.14")
        self.assertNotIn("source", packages["errno"])
        self.assertNotIn("source", packages["rama-tls-rustls"])
        self.assertIn("libc", packages["code-git-tooling"]["dependencies"])
        self.assertIn("code-rustwright-mcp", packages["code-browser"]["dependencies"])

    def setUp(self):
        fixture = test_artifact_contract.ArtifactContractTests()
        fixture.setUp()
        self.addCleanup(fixture.doCleanups)
        fixture.manifest.rename(fixture.directory / "code.com.json")
        self.output = fixture.directory
        self.commit = "a" * 40

    def test_dirty_source_cannot_build_or_publish(self):
        with patch.object(commands, "command", return_value=" M code-rs/core/src/openai_tools.rs"):
            with self.assertRaisesRegex(ValueError, "Commit source changes"):
                commands.require_clean_source()

    def test_auto_build_rejects_dirty_source_without_contacting_github(self):
        calls = []

        def run(args, **kwargs):
            calls.append(args)
            if args[:3] == ["git", "status", "--porcelain"]:
                return " M code-rs/core/src/openai_tools.rs"
            raise AssertionError(f"unexpected command: {args}")

        with patch.object(commands, "command", side_effect=run):
            with self.assertRaisesRegex(ValueError, "GitHub Actions cannot build uncommitted source"):
                commands.build_auto(self.output / "auto", self.commit)

        self.assertFalse(any(args[0] == "gh" for args in calls))

    def test_auto_build_delegates_only_to_github_actions(self):
        output = self.output / "auto"
        with patch.object(commands, "build_action") as build, \
             patch.object(commands, "build_local") as local_build:
            commands.build_auto(output, self.commit)

        build.assert_called_once_with(output, self.commit)
        local_build.assert_not_called()

    def test_actions_build_rechecks_source_state_before_dispatch(self):
        with patch.object(commands, "command", return_value=" M code-rs/core/src/openai_tools.rs") as run:
            with self.assertRaisesRegex(ValueError, "GitHub Actions cannot build uncommitted source"):
                commands.build_action(self.output / "actions", self.commit)
        run.assert_called_once_with(["git", "status", "--porcelain"], capture=True)

    def test_local_build_is_disabled(self):
        with patch.object(commands, "command") as run:
            with self.assertRaisesRegex(ValueError, "Local builds are disabled"):
                commands.build_local(self.output / "local")
        run.assert_not_called()

    def test_successful_workflow_with_skipped_windows_job_cannot_publish(self):
        run = {"headSha": self.commit, "conclusion": "success", "databaseId": 42}
        jobs = {"jobs": [
            {"name": "Build APE on Linux", "conclusion": "success"},
            {"name": "Run same APE on Windows", "conclusion": "skipped"},
        ]}
        with patch.object(commands, "workflow_runs", return_value=[run]), \
             patch.object(commands, "command", return_value=json.dumps(jobs)):
            with self.assertRaisesRegex(ValueError, "Linux and Windows"):
                commands.require_cross_platform_build(self.commit)

    def test_both_successful_platform_jobs_allow_publication(self):
        run = {"headSha": self.commit, "conclusion": "success", "databaseId": 42}
        jobs = {"jobs": [
            {"name": "Build APE on Linux", "conclusion": "success"},
            {"name": "Run same APE on Windows", "conclusion": "success"},
        ]}
        with patch.object(commands, "workflow_runs", return_value=[run]), \
             patch.object(commands, "command", return_value=json.dumps(jobs)):
            self.assertEqual(commands.require_cross_platform_build(self.commit), run)

    def test_run_forwards_arguments_and_preserves_exit_status(self):
        with patch.object(commands, "source_commit", return_value=self.commit), \
             patch.object(commands, "artifact_command", return_value=17) as run:
            self.assertEqual(commands.main(["run", "--output", str(self.output), "--", "exec", "hello world"]), 17)
            run.assert_called_once_with(self.output / "code.com", ["exec", "hello world"])

    def test_linux_ape_without_binfmt_uses_shell(self):
        with patch.object(commands.os, "name", "posix"), \
             patch.object(commands.subprocess, "run", side_effect=[
                 OSError(errno.ENOEXEC, "Exec format error"),
                 commands.subprocess.CompletedProcess([], 0),
             ]) as run:
            self.assertEqual(commands.artifact_command(self.output / "code.com", ["--version"]), 0)
            self.assertEqual(run.call_args.args[0], ["sh", str(self.output / "code.com"), "--version"])

    def test_popen_linux_ape_without_binfmt_uses_shell(self):
        expected = object()
        with patch.object(commands.os, "name", "posix"), \
             patch.object(commands.subprocess, "Popen", side_effect=[
                 OSError(errno.ENOEXEC, "Exec format error"),
                 expected,
             ]) as popen:
            actual = commands.artifact_process(
                commands.subprocess.Popen,
                self.output / "code.com",
                ["rustwright-mcp"],
                stdin=commands.subprocess.PIPE,
            )
        self.assertIs(actual, expected)
        self.assertEqual(
            popen.call_args.args[0],
            ["sh", str(self.output / "code.com"), "rustwright-mcp"],
        )

    def test_publish_rejects_an_executable_different_from_actions(self):
        manifest = commands.validate_artifact(*commands.artifact_files(self.output), expected_source_commit=self.commit)
        calls = []

        def download(args, **kwargs):
            calls.append(args)
            if args[0] == "git":
                return ""
            destination = Path(args[args.index("--dir") + 1])
            destination.mkdir()
            archive_size = manifest["browser_runtime"]["archive_size_bytes"]
            browser_archive = (self.output / "code.com").read_bytes()[-archive_size:]
            different = b"different tested executable" + browser_archive
            (destination / "code.com").write_bytes(different)
            changed = dict(
                manifest,
                artifact_sha256=hashlib.sha256(different).hexdigest(),
                artifact_size_bytes=len(different),
            )
            (destination / "code.com.json").write_text(json.dumps(changed), encoding="utf-8")
            return ""

        with patch.object(commands, "test_artifact", return_value=manifest), \
             patch.object(commands, "require_cross_platform_build", return_value={"databaseId": 42}), \
             patch.object(commands, "command", side_effect=download):
            with self.assertRaisesRegex(ValueError, "differs from the artifact tested"):
                commands.publish(self.output, self.commit, "test-tag", prerelease=True)
        self.assertFalse(any(args[:3] == ["gh", "release", "create"] for args in calls))

    def test_publish_rejects_a_tag_for_different_source(self):
        with patch.object(commands, "command", side_effect=["", "b" * 40 + "\trefs/tags/test-tag"]), \
             patch.object(commands, "test_artifact") as test:
            with self.assertRaisesRegex(ValueError, "different source commit"):
                commands.publish(self.output, self.commit, "test-tag", prerelease=True)
            test.assert_not_called()

    def test_prerelease_creation_and_stable_promotion_use_validated_assets(self):
        manifest = commands.validate_artifact(*commands.artifact_files(self.output), expected_source_commit=self.commit)
        for prerelease in (True, False):
            with self.subTest(prerelease=prerelease):
                calls = []

                def github(args, **kwargs):
                    calls.append(args)
                    if args[:3] == ["gh", "release", "view"]:
                        return json.dumps({"isPrerelease": True})
                    if "--dir" in args:
                        destination = Path(args[args.index("--dir") + 1])
                        destination.mkdir()
                        for source in commands.artifact_files(self.output):
                            shutil.copy2(source, destination / source.name)
                    return ""

                with patch.object(commands, "test_artifact", return_value=manifest), \
                     patch.object(commands, "require_cross_platform_build", return_value={"databaseId": 42, "url": "https://example.invalid/run/42"}), \
                     patch.object(commands, "command", side_effect=github):
                    commands.publish(self.output, self.commit, "test-tag", prerelease=prerelease)
                if prerelease:
                    create = next(args for args in calls if args[:3] == ["gh", "release", "create"])
                    self.assertIn("--prerelease", create)
                    self.assertIn("--latest=false", create)
                    self.assertEqual(create[create.index("--target") + 1], self.commit)
                    self.assertIn(str(self.output / "code.com"), create)
                else:
                    self.assertIn(["gh", "release", "edit", "test-tag", "--prerelease=false", "--latest"], calls)
                    self.assertFalse(any(args[:3] == ["gh", "release", "create"] for args in calls))

    def test_unknown_command_does_not_start_a_build(self):
        with patch.object(commands, "build_local") as build, redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                commands.main(["build"])
            build.assert_not_called()


if __name__ == "__main__":
    unittest.main()
