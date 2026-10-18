"""Hermetic stdio MCP fixture for operator-tool dispatch tests."""

import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        result = {
            "protocolVersion": request["params"]["protocolVersion"],
            "serverInfo": {"name": "operator-fixture", "version": "1.0.0"},
            "capabilities": {"tools": {}},
        }
    elif method == "tools/list":
        result = {"tools": [
            {"name": name, "description": "Fixture tool", "inputSchema": {
                "type": "object", "properties": {"text": {"type": "string"}},
                "required": ["text"],
            }} for name in ["echo", "disabled_echo"]
        ]}
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": request["params"]["arguments"]["text"]}]}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
