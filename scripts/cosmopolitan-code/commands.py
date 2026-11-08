#!/usr/bin/env python3
"""Build, validate, run, and publish the same Cosmopolitan executable."""

from __future__ import annotations

import argparse
import errno
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
from typing import Callable
from typing import TypeVar

from artifact_contract import (
    ArtifactContractError,
    validate_artifact,
    validate_development_artifact,
)


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = "cosmopolitan-code-ape.yml"
ARTIFACT = "cosmopolitan-code-ape"
ProcessResult = TypeVar("ProcessResult")
DEFAULT_BRANCH = "Main"
ORIGINAL_CODE_MOD_VERSION = (0, 6, 196)
MOD_VERSION_RE = re.compile(
    r"(?P<major>0|[1-9][0-9]*)\."
    r"(?P<minor>0|[1-9][0-9]*)\."
    r"(?P<patch>0|[1-9][0-9]*)b\Z"
)
DISPLAY_VERSION_RE = re.compile(
    r"codex version: (?P<codex>\S+) Mod version: (?P<mod>\S+)"
    r"(?: build attempts: (?P<attempt>[1-9][0-9]*))?\Z"
)


def command(args: list[str], *, capture: bool = False, **kwargs) -> str:
    result = subprocess.run(
        args, cwd=ROOT, check=True, text=True,
        stdout=subprocess.PIPE if capture else None, **kwargs,
    )
    return result.stdout.strip() if capture else ""


def source_commit() -> str:
    return command(["git", "rev-parse", "HEAD"], capture=True)


def require_clean_source() -> str:
    if command(["git", "status", "--porcelain"], capture=True):
        raise ValueError("Commit source changes before building or publishing.")
    return source_commit()


def output_path(value: str | None, commit: str) -> Path:
    value = value or os.environ.get("COSMOPOLITAN_CODE_OUTPUT")
    return Path(value).resolve() if value else ROOT / "o/cosmopolitan-code" / commit


def artifact_files(output: Path) -> tuple[Path, Path]:
    return output / "code.com", output / "code.com.json"


def artifact_process(
    factory: Callable[..., ProcessResult],
    artifact: Path,
    args: list[str],
    **kwargs,
) -> ProcessResult:
    command = [str(artifact), *args]
    try:
        return factory(command, **kwargs)
    except OSError as error:
        if os.name == "nt" or error.errno != errno.ENOEXEC:
            raise
        return factory(["sh", *command], **kwargs)


def artifact_command(artifact: Path, args: list[str]) -> int:
    if os.name != "nt":
        artifact.chmod(artifact.stat().st_mode | 0o111)
    return artifact_process(subprocess.run, artifact, args, cwd=ROOT).returncode


def artifact_output(artifact: Path, args: list[str]) -> str:
    if os.name != "nt":
        artifact.chmod(artifact.stat().st_mode | 0o111)
    result = artifact_process(
        subprocess.run,
        artifact,
        args,
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode:
        raise ValueError(f"Artifact startup failed: {' '.join(args)}")
    return result.stdout.strip()


def parse_display_version(output: str) -> dict[str, str | None]:
    match = DISPLAY_VERSION_RE.fullmatch(output.strip())
    if match is None:
        raise ValueError(f"Artifact reported an invalid version string: {output!r}")
    return match.groupdict()


def artifact_version(artifact: Path) -> dict[str, str | None]:
    return parse_display_version(artifact_output(artifact, ["--version"]))


def normalize_mod_version(value: str) -> str:
    normalized = value.strip()
    if normalized.startswith("v"):
        normalized = normalized[1:]
    match = MOD_VERSION_RE.fullmatch(normalized)
    if match is None:
        raise ValueError(
            "Mod version must use X.Y.Zb with the trailing b fork marker."
        )
    numeric = tuple(int(match.group(name)) for name in ("major", "minor", "patch"))
    if numeric <= ORIGINAL_CODE_MOD_VERSION:
        raise ValueError("Mod version must be newer than the original Code-mod 0.6.196 baseline.")
    return normalized


def normalize_build_attempt_count(value: str) -> str:
    normalized = value.strip()
    if not normalized.isascii() or not normalized.isdigit() or normalized.startswith("0"):
        raise ValueError("Build attempt count must be a positive integer.")
    return normalized


def dispatch_run_id(output: str) -> int | None:
    match = re.search(r"/actions/runs/(?P<run_id>[1-9][0-9]*)(?:/|\Z)", output.strip())
    return int(match.group("run_id")) if match is not None else None


def validate_requested_artifact_version(
    version: dict[str, str | None],
    *,
    mod_version: str | None,
    build_attempt_count: str | None,
) -> None:
    actual_mod = normalize_mod_version(version["mod"] or "")
    if mod_version is not None and actual_mod != normalize_mod_version(mod_version):
        raise ValueError("Actions artifact mod version does not match the requested build.")
    expected_attempt = (
        normalize_build_attempt_count(build_attempt_count)
        if build_attempt_count is not None
        else None
    )
    if version["attempt"] != expected_attempt:
        raise ValueError("Actions artifact build attempt does not match the requested build.")


def validate_output_artifact(output: Path, commit: str) -> dict:
    artifact, manifest = artifact_files(output)
    try:
        metadata = json.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ArtifactContractError(f"cannot read manifest {manifest}: {error}") from error
    validator = validate_artifact if metadata.get("source_clean") is True else validate_development_artifact
    return validator(artifact, manifest, expected_source_commit=commit)


def test_artifact(output: Path, commit: str) -> dict:
    artifact, _ = artifact_files(output)
    result = validate_output_artifact(output, commit)
    artifact_version(artifact)
    if artifact_command(artifact, ["--help"]):
        raise ValueError("Artifact startup failed: --help")
    return result


def workflow_runs(commit: str) -> list[dict]:
    return json.loads(command([
        "gh", "run", "list", "--workflow", WORKFLOW,
        "--branch", DEFAULT_BRANCH, "--commit", commit, "--limit", "100",
        "--json", "databaseId,headSha,status,conclusion,url,event",
    ], capture=True))


def require_cross_platform_build(
    commit: str,
    expected_artifact_sha256: str | None = None,
) -> dict:
    for run in workflow_runs(commit):
        if run["headSha"] == commit and run["conclusion"] == "success":
            jobs = json.loads(command(["gh", "run", "view", str(run["databaseId"]),
                                       "--json", "jobs"], capture=True))["jobs"]
            completed = {job["name"] for job in jobs if job["conclusion"] == "success"}
            if {"Build APE on Linux", "Run same APE on Windows"} <= completed:
                if expected_artifact_sha256 is None:
                    return run
                with tempfile.TemporaryDirectory(prefix="cosmopolitan-tested-") as temp:
                    tested = Path(temp) / "tested"
                    try:
                        command([
                            "gh", "run", "download", str(run["databaseId"]),
                            "--name", ARTIFACT, "--dir", str(tested),
                        ])
                        tested_artifact, tested_manifest = artifact_files(tested)
                        metadata = validate_artifact(
                            tested_artifact,
                            tested_manifest,
                            expected_source_commit=commit,
                        )
                    except (ArtifactContractError, OSError, subprocess.CalledProcessError):
                        continue
                    if metadata["artifact_sha256"] == expected_artifact_sha256:
                        return run
    if expected_artifact_sha256 is not None:
        raise ValueError(
            "Publishing requires a successful Linux and Windows Actions run for "
            "these exact artifact bytes."
        )
    raise ValueError("Publishing requires a successful APE Linux and Windows Actions run for this commit.")


def build_action(
    output: Path,
    commit: str,
    *,
    mod_version: str | None = None,
    build_attempt_count: str | None = None,
) -> None:
    if command(["git", "status", "--porcelain"], capture=True):
        raise ValueError(
            "GitHub Actions cannot build uncommitted source until the exact-source "
            "snapshot workflow is committed; local builds are disabled for this repository."
        )
    if source_commit() != commit:
        raise ValueError("Actions can only build the unchanged committed source revision.")
    if command(["git", "branch", "--show-current"], capture=True) != DEFAULT_BRANCH:
        raise ValueError(f"Actions builds are restricted to {DEFAULT_BRANCH}.")
    remote = command(["git", "ls-remote", "origin", f"refs/heads/{DEFAULT_BRANCH}"], capture=True)
    if not remote or remote.split()[0] != commit:
        raise ValueError(f"Push this commit to origin/{DEFAULT_BRANCH} before building it on Actions.")
    if output.exists():
        raise ValueError(f"Refusing to overwrite artifact directory: {output}")
    workflow_args = ["gh", "workflow", "run", WORKFLOW, "--ref", DEFAULT_BRANCH]
    if mod_version is not None:
        workflow_args.extend(["-f", f"mod_version={normalize_mod_version(mod_version)}"])
    if build_attempt_count is not None:
        workflow_args.extend([
            "-f",
            f"build_attempt_count={normalize_build_attempt_count(build_attempt_count)}",
        ])
    previous_ids = {run["databaseId"] for run in workflow_runs(commit)}
    dispatch_output = command(workflow_args, capture=True)
    run_id = dispatch_run_id(dispatch_output)
    if run_id is not None:
        run = {"databaseId": run_id, "url": dispatch_output.strip()}
    else:
        deadline = time.monotonic() + 120
        while time.monotonic() < deadline:
            runs = [run for run in workflow_runs(commit)
                    if run["databaseId"] not in previous_ids and run["event"] == "workflow_dispatch"]
            if runs:
                run = runs[0]
                break
            time.sleep(5)
        else:
            raise ValueError("Actions did not report the dispatched build within 120 seconds.")
    print(run["url"], flush=True)
    command(["gh", "run", "watch", str(run["databaseId"]), "--exit-status"])
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="cosmopolitan-download-", dir=output.parent) as temp:
        staged = Path(temp) / "artifact"
        command(["gh", "run", "download", str(run["databaseId"]),
                 "--name", ARTIFACT, "--dir", str(staged)])
        test_artifact(staged, commit)
        validate_requested_artifact_version(
            artifact_version(staged / "code.com"),
            mod_version=mod_version,
            build_attempt_count=build_attempt_count,
        )
        staged.rename(output)
    print(f"Artifact: {output / 'code.com'}", flush=True)


def build_local(output: Path) -> None:
    del output
    raise ValueError(
        "Local builds are disabled for this repository. Use GitHub Actions against the exact "
        "intended source revision."
    )


def build_auto(output: Path, commit: str) -> None:
    build_action(output, commit)


def publish(output: Path, commit: str, tag: str, *, prerelease: bool) -> None:
    if tag.startswith("-"):
        raise ValueError("A release tag cannot start with a hyphen.")
    tag_mod_version = normalize_mod_version(tag)
    command(["git", "check-ref-format", f"refs/tags/{tag}"])
    refs = command(["git", "ls-remote", "--tags", "origin",
                    f"refs/tags/{tag}", f"refs/tags/{tag}^{{}}"], capture=True)
    tag_refs = dict(line.split()[::-1] for line in refs.splitlines())
    tag_commit = tag_refs.get(f"refs/tags/{tag}^{{}}", tag_refs.get(f"refs/tags/{tag}"))
    if tag_commit is not None and tag_commit != commit:
        raise ValueError("Release tag already refers to a different source commit.")
    manifest = test_artifact(output, commit)
    artifact, manifest_path = artifact_files(output)
    version = artifact_version(artifact)
    if version["mod"] != tag_mod_version:
        raise ValueError("Artifact mod version does not match the release tag.")
    if prerelease and version["attempt"] is None:
        raise ValueError("Prerelease artifacts must report their build attempt count.")
    if not prerelease and version["attempt"] is not None:
        raise ValueError("Stable release artifacts must not report a build attempt count.")
    run = require_cross_platform_build(commit, manifest["artifact_sha256"])
    with tempfile.TemporaryDirectory(prefix="cosmopolitan-release-") as temp:
        if prerelease:
            notes = Path(temp) / "notes.md"
            summary = command(["git", "log", "-1", "--format=%s"], capture=True)
            notes.write_text(
                "One x86-64 Cosmopolitan APE executable (`code.com`) for Windows and Linux.\n\n"
                f"Source: `{commit}`. Linux and Windows startup checks: {run['url']}\n\n"
                f"{summary}\n\n"
                "The adjacent manifest records the source and SDK pins and executable SHA-256.\n"
                "This experimental APE build supports the normal command execution paths. "
                "Native file pickers, file-manager reveal, and clipboard-image paste remain unavailable.\n",
                encoding="utf-8",
            )
            command(["gh", "release", "create", tag, str(artifact), str(manifest_path),
                     "--target", commit, "--title", f"Cosmopolitan Code {tag}",
                     "--notes-file", str(notes), "--prerelease", "--latest=false"])
        else:
            release = json.loads(command(["gh", "release", "view", tag,
                                           "--json", "isPrerelease"], capture=True))
            if not release["isPrerelease"]:
                raise ValueError("publish-release requires an existing prerelease to promote.")
            released = Path(temp) / "released"
            command(["gh", "release", "download", tag, "--pattern", "code.com*", "--dir", str(released)])
            released_artifact, released_manifest = artifact_files(released)
            validate_artifact(released_artifact, released_manifest, expected_source_commit=commit)
            prerelease_version = artifact_version(released_artifact)
            if prerelease_version["attempt"] is None:
                raise ValueError("Existing prerelease does not report a build attempt count.")
            if prerelease_version["codex"] != version["codex"]:
                raise ValueError("Stable artifact Codex version differs from the prerelease.")
            if prerelease_version["mod"] != version["mod"]:
                raise ValueError("Stable artifact mod version differs from the prerelease.")
            command([
                "gh", "release", "upload", tag, str(artifact), str(manifest_path),
                "--clobber",
            ])
            command(["gh", "release", "edit", tag, "--prerelease=false", "--latest"])


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="command", required=True)
    for name, help_text in (
        ("build-auto", "Build through GitHub Actions for the exact committed revision."),
        ("build-action", "Build on GitHub and test the same executable on Linux and Windows."),
        ("build-local", "Explain that local builds are disabled for this repository."),
        ("test", "Check the manifest, source commit, hash, and executable startup."),
        ("run", "Run the validated executable with CLI arguments."),
        ("publish-prerelease", "Publish the artifact tested on Linux and Windows as a prerelease."),
        ("publish-release", "Promote an existing matching prerelease to the latest stable release."),
    ):
        item = commands.add_parser(name, help=help_text)
        item.add_argument("--output", help="Artifact directory (default: o/cosmopolitan-code/<commit>).")
        if name.startswith("publish-"):
            item.add_argument("--tag", required=True, help="GitHub release tag.")
        if name == "build-action":
            item.add_argument(
                "--mod-version",
                help="Code-mod version to embed (a leading v is removed).",
            )
            item.add_argument(
                "--build-attempt-count",
                help="Positive workflow attempt count to expose in prerelease output.",
            )
        if name == "run":
            item.add_argument("args", nargs=argparse.REMAINDER, help="Arguments after -- go to code.com.")
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        commit = (
            require_clean_source()
            if args.command == "build-action" or args.command.startswith("publish-")
            else source_commit()
        )
        output = output_path(args.output, commit)
        if args.command == "build-auto":
            build_auto(output, commit)
        elif args.command == "build-action":
            build_action(
                output,
                commit,
                mod_version=args.mod_version,
                build_attempt_count=args.build_attempt_count,
            )
        elif args.command == "build-local":
            build_local(output)
        elif args.command == "test":
            test_artifact(output, commit)
        elif args.command == "run":
            artifact, _ = artifact_files(output)
            validate_output_artifact(output, commit)
            forwarded = args.args[1:] if args.args[:1] == ["--"] else args.args
            return artifact_command(artifact, forwarded)
        else:
            publish(output, commit, args.tag, prerelease=args.command == "publish-prerelease")
    except (ArtifactContractError, ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
