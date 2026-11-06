#!/usr/bin/env python3
"""Smoke-test the Rustwright MCP server embedded in a Code executable."""

from __future__ import annotations

import argparse
import base64
import collections
import json
import os
import queue
import re
import subprocess
import sys
import threading
import time
import urllib.parse
import zlib
from pathlib import Path
from typing import Any

from commands import artifact_process


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


def expect_response(
    lines: queue.Queue[str], request_id: int, timeout: float = 30.0
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while True:
        response = receive(lines, max(0.01, deadline - time.monotonic()))
        if "id" not in response:
            continue
        if response.get("id") != request_id:
            raise RuntimeError(f"unexpected MCP response id: {response!r}")
        if "error" in response:
            raise RuntimeError(f"MCP request {request_id} failed: {response['error']!r}")
        return response


def require_tool(
    tools: dict[str, dict[str, Any]], name: str, properties: set[str]
) -> str:
    tool = tools.get(name)
    if tool is None:
        raise RuntimeError(f"Rustwright MCP tool missing: {name}")
    schema = tool.get("inputSchema")
    if not isinstance(schema, dict) or schema.get("type") != "object":
        raise RuntimeError(f"{name} returned an invalid input schema: {schema!r}")
    actual = schema.get("properties")
    if not isinstance(actual, dict):
        raise RuntimeError(f"{name} returned no schema properties: {schema!r}")
    if missing := properties - actual.keys():
        raise RuntimeError(f"{name} schema is missing properties: {sorted(missing)!r}")
    return str(tool["name"])


def result_content(response: dict[str, Any]) -> list[dict[str, Any]]:
    result = response.get("result")
    if not isinstance(result, dict):
        raise RuntimeError(f"MCP tool returned no result object: {response!r}")
    if result.get("isError"):
        raise RuntimeError(f"MCP tool failed: {response!r}")
    content = result.get("content")
    if not isinstance(content, list):
        raise RuntimeError(f"MCP tool returned no content: {response!r}")
    return [item for item in content if isinstance(item, dict)]


def result_text(response: dict[str, Any]) -> str:
    return "\n".join(
        str(item.get("text", ""))
        for item in result_content(response)
        if item.get("type") == "text"
    )


def call_tool(
    process: subprocess.Popen[str],
    lines: queue.Queue[str],
    request_id: int,
    name: str,
    arguments: dict[str, Any],
    timeout: float = 90.0,
) -> dict[str, Any]:
    send(
        process,
        {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        },
    )
    response = expect_response(lines, request_id, timeout)
    result_content(response)
    return response


def paeth(left: int, above: int, upper_left: int) -> int:
    estimate = left + above - upper_left
    left_distance = abs(estimate - left)
    above_distance = abs(estimate - above)
    upper_left_distance = abs(estimate - upper_left)
    if left_distance <= above_distance and left_distance <= upper_left_distance:
        return left
    if above_distance <= upper_left_distance:
        return above
    return upper_left


def validate_nonuniform_png(data: bytes) -> dict[str, Any]:
    if not data.startswith(b"\x89PNG\r\n\x1a\n"):
        raise RuntimeError("browser screenshot is not a PNG")
    offset = 8
    header: tuple[int, int, int, int, int] | None = None
    compressed = bytearray()
    while offset + 12 <= len(data):
        length = int.from_bytes(data[offset : offset + 4], "big")
        kind = data[offset + 4 : offset + 8]
        payload_start = offset + 8
        payload_end = payload_start + length
        if payload_end + 4 > len(data):
            raise RuntimeError("browser screenshot has a truncated PNG chunk")
        payload = data[payload_start:payload_end]
        if kind == b"IHDR":
            if len(payload) != 13:
                raise RuntimeError("browser screenshot has an invalid PNG header")
            width = int.from_bytes(payload[0:4], "big")
            height = int.from_bytes(payload[4:8], "big")
            header = (width, height, payload[8], payload[9], payload[12])
        elif kind == b"IDAT":
            compressed.extend(payload)
        elif kind == b"IEND":
            break
        offset = payload_end + 4
    if header is None or not compressed:
        raise RuntimeError("browser screenshot lacks PNG image data")
    width, height, bit_depth, color_type, interlace = header
    if width < 1 or height < 1:
        raise RuntimeError("browser screenshot has empty dimensions")
    if bit_depth != 8 or color_type not in {2, 6} or interlace != 0:
        raise RuntimeError(
            "browser screenshot uses an unsupported PNG encoding "
            f"(bitDepth={bit_depth}, colorType={color_type}, interlace={interlace})"
        )
    channels = 3 if color_type == 2 else 4
    stride = width * channels
    raw = zlib.decompress(bytes(compressed))
    if len(raw) != height * (stride + 1):
        raise RuntimeError("browser screenshot PNG scanline size is inconsistent")
    previous = bytearray(stride)
    colors: collections.Counter[tuple[int, int, int]] = collections.Counter()
    cursor = 0
    for _ in range(height):
        filter_type = raw[cursor]
        cursor += 1
        encoded = raw[cursor : cursor + stride]
        cursor += stride
        decoded = bytearray(stride)
        for index, value in enumerate(encoded):
            left = decoded[index - channels] if index >= channels else 0
            above = previous[index]
            upper_left = previous[index - channels] if index >= channels else 0
            if filter_type == 0:
                decoded[index] = value
            elif filter_type == 1:
                decoded[index] = (value + left) & 0xFF
            elif filter_type == 2:
                decoded[index] = (value + above) & 0xFF
            elif filter_type == 3:
                decoded[index] = (value + ((left + above) // 2)) & 0xFF
            elif filter_type == 4:
                decoded[index] = (value + paeth(left, above, upper_left)) & 0xFF
            else:
                raise RuntimeError(f"browser screenshot uses invalid PNG filter {filter_type}")
        for index in range(0, stride, channels):
            colors[(decoded[index], decoded[index + 1], decoded[index + 2])] += 1
        previous = decoded
    pixel_count = width * height
    dominant = max(colors.values())
    dominant_ratio = dominant / pixel_count
    if len(colors) < 2 or dominant_ratio >= 0.98:
        raise RuntimeError(
            "browser screenshot is visually uniform "
            f"({len(colors)} colors, {dominant_ratio:.2%} dominant)"
        )
    return {
        "width": width,
        "height": height,
        "uniqueColors": len(colors),
        "dominantRatio": dominant_ratio,
    }


def write_atomic(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_bytes(data)
    os.replace(temporary, path)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("executable")
    parser.add_argument("--exercise-browser", action="store_true")
    parser.add_argument("--report", type=Path)
    parser.add_argument("--screenshot-output", type=Path)
    args = parser.parse_args()

    process = artifact_process(
        subprocess.Popen,
        Path(args.executable),
        ["rustwright-mcp"],
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
        tool_list = listed["result"]["tools"]
        tools = {tool["name"]: tool for tool in tool_list}
        navigate_name = require_tool(tools, "browser_navigate", {"url"})
        snapshot_name = require_tool(tools, "browser_snapshot", {"target", "depth", "boxes"})
        tabs_name = require_tool(tools, "browser_tabs", {"action", "index", "url"})
        close_name = require_tool(tools, "browser_close", set())
        report: dict[str, Any] = {
            "server": initialized["result"]["serverInfo"],
            "tools": sorted(tools),
            "browser": None,
        }

        next_id = 3
        if args.exercise_browser:
            resize_name = require_tool(tools, "browser_resize", {"width", "height"})
            evaluate_name = require_tool(
                tools, "browser_evaluate", {"function", "target", "element"}
            )
            click_name = require_tool(
                tools, "browser_click", {"target", "doubleClick", "element"}
            )
            screenshot_name = require_tool(
                tools, "browser_take_screenshot", {"fullPage", "type"}
            )
            html = """<!doctype html><meta charset=utf-8><title>Code Rustwright CI</title>
<style>
html,body{margin:0;min-height:100%;font:24px system-ui;background:linear-gradient(135deg,#102a43,#0ea5e9);color:#f8fafc}
main{width:min(720px,80vw);margin:12vh auto;padding:48px;border-radius:28px;background:#ffffff18;box-shadow:0 30px 80px #0006}
button{font:inherit;padding:16px 28px;border:0;border-radius:14px;background:#fbbf24;color:#422006;cursor:pointer}
output{display:block;margin-top:24px;color:#d9f99d}
</style><main><h1>Rustwright native runtime</h1><p id=dimensions>ready</p>
<button id=activate onclick="document.querySelector('output').textContent='activated';this.dataset.clicked='yes';const popup=window.open('about:blank','_blank');popup.document.title='Code Popup';popup.document.body.innerHTML='<main>popup ready</main>'">Activate test</button>
<output>idle</output></main>"""
            url = "data:text/html," + urllib.parse.quote(html, safe="")
            navigated = call_tool(
                process, lines, next_id, navigate_name, {"url": url}, timeout=120.0
            )
            next_id += 1
            if "Rustwright native runtime" not in result_text(navigated):
                raise RuntimeError("browser_navigate snapshot omitted the test page")

            resized = call_tool(
                process,
                lines,
                next_id,
                resize_name,
                {"width": 1024, "height": 768},
                timeout=120.0,
            )
            next_id += 1
            resize_snapshot = result_text(resized)
            button_match = re.search(
                r"button[^\n]*\[ref=(e[1-9][0-9]*)\]", resize_snapshot
            )
            if button_match is None:
                button_match = re.search(
                    r"\[ref=(e[1-9][0-9]*)\][\s\S]{0,160}Activate test",
                    resize_snapshot,
                )
            if button_match is None:
                raise RuntimeError(f"could not find the button ref in snapshot: {resize_snapshot}")
            button_ref = button_match.group(1)

            clicked = call_tool(
                process,
                lines,
                next_id,
                click_name,
                {"target": button_ref, "doubleClick": False, "element": "Activate test"},
            )
            next_id += 1
            if "activated" not in result_text(clicked):
                raise RuntimeError("browser_click did not update the page")

            evaluated = call_tool(
                process,
                lines,
                next_id,
                evaluate_name,
                {
                    "function": "() => { const canvas = document.createElement('canvas'); const gl = canvas.getContext('webgl'); const debug = gl && gl.getExtension('WEBGL_debug_renderer_info'); return {title: document.title, width: innerWidth, height: innerHeight, clicked: document.querySelector('#activate').dataset.clicked, output: document.querySelector('output').textContent, fingerprint: {userAgent: navigator.userAgent, webdriver: navigator.webdriver ?? false, hardwareConcurrency: navigator.hardwareConcurrency, screenWidth: screen.width, screenHeight: screen.height, historyLength: history.length, webglVendor: debug ? gl.getParameter(debug.UNMASKED_VENDOR_WEBGL) : '', webglRenderer: debug ? gl.getParameter(debug.UNMASKED_RENDERER_WEBGL) : ''}}; }",
                    "target": None,
                    "element": None,
                },
            )
            next_id += 1
            evaluation = json.loads(result_text(evaluated))
            expected_evaluation = {
                "title": "Code Rustwright CI",
                "width": 1024,
                "height": 768,
                "clicked": "yes",
                "output": "activated",
            }
            if {key: evaluation.get(key) for key in expected_evaluation} != expected_evaluation:
                raise RuntimeError(
                    f"browser evaluation mismatch: {evaluation!r} != {expected_evaluation!r}"
                )
            fingerprint = evaluation.get("fingerprint")
            if not isinstance(fingerprint, dict):
                raise RuntimeError(f"Camoufox fingerprint surfaces are missing: {evaluation!r}")
            if "Firefox/" not in str(fingerprint.get("userAgent", "")):
                raise RuntimeError(f"Camoufox user agent was not applied: {fingerprint!r}")
            if fingerprint.get("webdriver") is not False:
                raise RuntimeError(f"navigator.webdriver leaked automation: {fingerprint!r}")
            for field in ("hardwareConcurrency", "screenWidth", "screenHeight", "historyLength"):
                if not isinstance(fingerprint.get(field), (int, float)) or fingerprint[field] <= 0:
                    raise RuntimeError(f"Camoufox fingerprint field is invalid: {field}={fingerprint.get(field)!r}")
            for field in ("webglVendor", "webglRenderer"):
                if not str(fingerprint.get(field, "")).strip():
                    raise RuntimeError(f"Camoufox WebGL fingerprint is missing: {fingerprint!r}")

            tabs_text = ""
            popup_deadline = time.monotonic() + 10.0
            while time.monotonic() < popup_deadline:
                tabs = call_tool(
                    process,
                    lines,
                    next_id,
                    tabs_name,
                    {"action": "list"},
                )
                next_id += 1
                tabs_text = result_text(tabs)
                if "1:" in tabs_text:
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError(f"popup-created page was not exposed by browser_tabs: {tabs_text}")

            popup = call_tool(
                process,
                lines,
                next_id,
                tabs_name,
                {"action": "select", "index": 1},
            )
            next_id += 1
            if "popup ready" not in result_text(popup):
                raise RuntimeError("popup tab could not be selected through the packaged Juggler API")
            call_tool(
                process,
                lines,
                next_id,
                tabs_name,
                {"action": "close", "index": 1},
            )
            next_id += 1

            snapshotted = call_tool(process, lines, next_id, snapshot_name, {})
            next_id += 1
            if "activated" not in result_text(snapshotted):
                raise RuntimeError("browser_snapshot omitted the post-interaction state")

            screenshot = call_tool(
                process,
                lines,
                next_id,
                screenshot_name,
                {"fullPage": False, "type": "png"},
                timeout=120.0,
            )
            next_id += 1
            image_blocks = [
                item for item in result_content(screenshot) if item.get("type") == "image"
            ]
            if len(image_blocks) != 1:
                raise RuntimeError(f"expected one inline browser screenshot: {screenshot!r}")
            screenshot_bytes = base64.b64decode(image_blocks[0]["data"], validate=True)
            screenshot_report = validate_nonuniform_png(screenshot_bytes)
            if args.screenshot_output is not None:
                write_atomic(args.screenshot_output, screenshot_bytes)
            report["browser"] = {
                "navigation": "passed",
                "viewport": {"width": 1024, "height": 768},
                "interaction": "passed",
                "evaluation": evaluation,
                "camouflags": fingerprint,
                "popupApi": "passed",
                "screenshot": screenshot_report,
            }

        call_tool(process, lines, next_id, close_name, {})

        if args.report is not None:
            write_atomic(
                args.report,
                (json.dumps(report, indent=2, sort_keys=True) + "\n").encode("utf-8"),
            )

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
