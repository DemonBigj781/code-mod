#!/usr/bin/env python3
"""Build, validate, run, and publish the same Cosmopolitan executable."""

from __future__ import annotations

import argparse
import errno
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

from artifact_contract import ArtifactContractError, validate_artifact


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = "cosmopolitan-code-ape.yml"
ARTIFACT = "cosmopolitan-code-ape"
DEFAULT_BRANCH = "Main"


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


def artifact_command(artifact: Path, args: list[str]) -> int:
    if os.name != "nt":
        artifact.chmod(artifact.stat().st_mode | 0o111)
    try:
        return subprocess.run([str(artifact), *args], cwd=ROOT).returncode
    except OSError as error:
        if os.name == "nt" or error.errno != errno.ENOEXEC:
            raise
        return subprocess.run(["sh", str(artifact), *args], cwd=ROOT).returncode


def test_artifact(output: Path, commit: str) -> dict:
    artifact, manifest = artifact_files(output)
    result = validate_artifact(artifact, manifest, expected_source_commit=commit)
    for args in (["--version"], ["--help"]):
        if artifact_command(artifact, args):
            raise ValueError(f"Artifact startup failed: {' '.join(args)}")
    return result


def workflow_runs(commit: str) -> list[dict]:
    return json.loads(command([
        "gh", "run", "list", "--workflow", WORKFLOW,
        "--branch", DEFAULT_BRANCH, "--commit", commit, "--limit", "100",
        "--json", "databaseId,headSha,status,conclusion,url,event",
    ], capture=True))


def require_cross_platform_build(commit: str) -> dict:
    for run in workflow_runs(commit):
        if run["headSha"] == commit and run["conclusion"] == "success":
            jobs = json.loads(command(["gh", "run", "view", str(run["databaseId"]),
                                       "--json", "jobs"], capture=True))["jobs"]
            completed = {job["name"] for job in jobs if job["conclusion"] == "success"}
            if {"Build APE on Linux", "Run same APE on Windows"} <= completed:
                return run
    raise ValueError("Publishing requires a successful APE Linux and Windows Actions run for this commit.")


def build_action(output: Path, commit: str) -> None:
    if command(["git", "branch", "--show-current"], capture=True) != DEFAULT_BRANCH:
        raise ValueError(f"Actions builds are restricted to {DEFAULT_BRANCH}.")
    remote = command(["git", "ls-remote", "origin", f"refs/heads/{DEFAULT_BRANCH}"], capture=True)
    if not remote or remote.split()[0] != commit:
        raise ValueError(f"Push this commit to origin/{DEFAULT_BRANCH} before building it on Actions.")
    if output.exists():
        raise ValueError(f"Refusing to overwrite artifact directory: {output}")
    previous_ids = {run["databaseId"] for run in workflow_runs(commit)}
    command(["gh", "workflow", "run", WORKFLOW, "--ref", DEFAULT_BRANCH])
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
        staged.rename(output)
    print(f"Artifact: {output / 'code.com'}", flush=True)


def build_local(output: Path) -> None:
    if output.exists():
        raise ValueError(f"Refusing to overwrite artifact directory: {output}")
    source = ROOT / "o/cosmopolitan-source"
    if not source.exists():
        pins = json.loads((Path(__file__).with_name("pins.json")).read_text(encoding="utf-8"))
        source.parent.mkdir(parents=True, exist_ok=True)
        command(["git", "clone", "--filter=blob:none", "--no-checkout",
                 pins["cosmopolitan_repository"], str(source)])
        command(["git", "-C", str(source), "checkout", "--detach", pins["cosmopolitan_commit"]])
    env = os.environ.copy()
    env.update(COSMOPOLITAN_CODE_OUTPUT=str(output), CARGO_BUILD_JOBS="1",
               CMAKE_BUILD_PARALLEL_LEVEL="1", MAKEFLAGS="-j1")
    command(["sh", str(ROOT / "scripts/build-cosmopolitan-code.sh")], env=env)


def publish(output: Path, commit: str, tag: str, *, prerelease: bool) -> None:
    if tag.startswith("-"):
        raise ValueError("A release tag cannot start with a hyphen.")
    command(["git", "check-ref-format", f"refs/tags/{tag}"])
    refs = command(["git", "ls-remote", "--tags", "origin",
                    f"refs/tags/{tag}", f"refs/tags/{tag}^{{}}"], capture=True)
    tag_refs = dict(line.split()[::-1] for line in refs.splitlines())
    tag_commit = tag_refs.get(f"refs/tags/{tag}^{{}}", tag_refs.get(f"refs/tags/{tag}"))
    if tag_commit is not None and tag_commit != commit:
        raise ValueError("Release tag already refers to a different source commit.")
    manifest = test_artifact(output, commit)
    run = require_cross_platform_build(commit)
    artifact, manifest_path = artifact_files(output)
    # Compare the proposed asset with the artifact tested on both hosted runners.
    with tempfile.TemporaryDirectory(prefix="cosmopolitan-release-") as temp:
        tested = Path(temp) / "tested"
        command(["gh", "run", "download", str(run["databaseId"]),
                 "--name", ARTIFACT, "--dir", str(tested)])
        tested_artifact, tested_manifest = artifact_files(tested)
        expected = validate_artifact(tested_artifact, tested_manifest, expected_source_commit=commit)
        if expected["artifact_sha256"] != manifest["artifact_sha256"]:
            raise ValueError("Proposed executable differs from the artifact tested on Linux and Windows.")
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
            published = validate_artifact(released_artifact, released_manifest, expected_source_commit=commit)
            if published["artifact_sha256"] != manifest["artifact_sha256"]:
                raise ValueError("Prerelease executable differs from the validated artifact.")
            command(["gh", "release", "edit", tag, "--prerelease=false", "--latest"])


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="command", required=True)
    for name, help_text in (
        ("build-action", "Build on GitHub and test the same executable on Linux and Windows."),
        ("build-local", "Build locally with one compiler worker."),
        ("test", "Check the manifest, source commit, hash, and executable startup."),
        ("run", "Run the validated executable with CLI arguments."),
        ("publish-prerelease", "Publish the artifact tested on Linux and Windows as a prerelease."),
        ("publish-release", "Promote an existing matching prerelease to the latest stable release."),
    ):
        item = commands.add_parser(name, help=help_text)
        item.add_argument("--output", help="Artifact directory (default: o/cosmopolitan-code/<commit>).")
        if name.startswith("publish-"):
            item.add_argument("--tag", required=True, help="GitHub release tag.")
        if name == "run":
            item.add_argument("args", nargs=argparse.REMAINDER, help="Arguments after -- go to code.com.")
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        commit = require_clean_source() if args.command.startswith(("build-", "publish-")) else source_commit()
        output = output_path(args.output, commit)
        if args.command == "build-action":
            build_action(output, commit)
        elif args.command == "build-local":
            build_local(output)
        elif args.command == "test":
            test_artifact(output, commit)
        elif args.command == "run":
            artifact, manifest = artifact_files(output)
            validate_artifact(artifact, manifest, expected_source_commit=commit)
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
