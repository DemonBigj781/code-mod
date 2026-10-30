#!/usr/bin/env python3
"""Smoke-test the Rustwright MCP server embedded in a Code executable."""

from __future__ import annotations

import argparse
import json
import os
import queue
import subprocess
import sys
import threading
import time
from typing import Any


def send(process: subprocess.Popen[str], message: dict[str, Any]) -> None:
    assert process.stdin is not None
    process.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
    process.stdin.flush()


def receive(lines: queue.Queue[str], timeout: float = 30.0) -> dict[str, Any]:
    try:
        line = lines.get(timeout=timeout)
    except queue.Empty as error:
        raise TimeoutError("timed out waiting for Rustwright MCP response") from error
    if not line:
        raise RuntimeError("Rustwright MCP stdout closed before a response")
    return json.loads(line)


def expect_response(lines: queue.Queue[str], request_id: int) -> dict[str, Any]:
    response = receive(lines)
    if response.get("id") != request_id:
        raise RuntimeError(f"unexpected MCP response id: {response!r}")
    if "error" in response:
        raise RuntimeError(f"MCP request {request_id} failed: {response['error']!r}")
    return response


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("executable")
    parser.add_argument("--exercise-browser", action="store_true")
    args = parser.parse_args()

    process = subprocess.Popen(
        [args.executable, "rustwright-mcp"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        env=os.environ.copy(),
    )
    stdout = process.stdout
    assert stdout is not None
    lines: queue.Queue[str] = queue.Queue()

    def read_stdout() -> None:
        for line in stdout:
            lines.put(line)
        lines.put("")

    threading.Thread(target=read_stdout, name="rustwright-mcp-stdout", daemon=True).start()
    try:
        send(
            process,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "code-ci", "version": "0"},
                },
            },
        )
        initialized = expect_response(lines, 1)
        server_name = initialized["result"]["serverInfo"]["name"]
        if server_name != "rustwright-mcp":
            raise RuntimeError(f"unexpected MCP server name: {server_name!r}")

        send(process, {"jsonrpc": "2.0", "method": "notifications/initialized"})
        send(
            process,
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
        )
        listed = expect_response(lines, 2)
        tool_names = {tool["name"] for tool in listed["result"]["tools"]}
        required = {"browser_navigate", "browser_snapshot", "browser_close"}
        if missing := required - tool_names:
            raise RuntimeError(f"Rustwright MCP tools missing: {sorted(missing)!r}")

        next_id = 3
        if args.exercise_browser:
            send(
                process,
                {
                    "jsonrpc": "2.0",
                    "id": next_id,
                    "method": "tools/call",
                    "params": {
                        "name": "browser_navigate",
                        "arguments": {
                            "url": "data:text/html,<title>Code Rustwright CI</title><h1>ready</h1>"
                        },
                    },
                },
            )
            navigated = expect_response(lines, next_id)
            if navigated["result"].get("isError"):
                raise RuntimeError(f"browser_navigate failed: {navigated!r}")
            next_id += 1

        send(
            process,
            {
                "jsonrpc": "2.0",
                "id": next_id,
                "method": "tools/call",
                "params": {"name": "browser_close", "arguments": {}},
            },
        )
        closed = expect_response(lines, next_id)
        if closed["result"].get("isError"):
            raise RuntimeError(f"browser_close failed: {closed!r}")

        assert process.stdin is not None
        process.stdin.close()
        deadline = time.monotonic() + 15.0
        while process.poll() is None and time.monotonic() < deadline:
            time.sleep(0.05)
        if process.poll() is None:
            raise TimeoutError("Rustwright MCP did not exit after stdin closed")
        if process.returncode != 0:
            raise RuntimeError(f"Rustwright MCP exited with {process.returncode}")
        return 0
    except Exception as error:
        process.kill()
        process.wait()
        diagnostics = process.stderr.read() if process.stderr is not None else ""
        print(f"Rustwright MCP smoke test failed: {error}", file=sys.stderr)
        if diagnostics:
            print(diagnostics, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
