"""Exercise the shipped native history worker without credentials or real stores."""

import errno
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def run_worker(artifact, payload, env):
    command = [str(artifact), "--code-session-history-worker"]
    try:
        result = subprocess.run(command, input=payload, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, env=env, timeout=40, check=False)
    except OSError as error:
        if os.name != "posix" or error.errno != errno.ENOEXEC:
            raise
        result = subprocess.run(["sh", *command], input=payload, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, env=env, timeout=40, check=False)
    return result


def invoke(artifact, payload, env):
    result = run_worker(artifact, payload, env)
    assert result.returncode == 0, (result.returncode, result.stderr.decode(errors="replace"))
    assert len(result.stdout) <= 4 * 1024 * 1024
    return json.loads(result.stdout)


def main():
    artifact = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="code-history-worker-") as temporary:
        root = Path(temporary)
        store = root / "history"
        sessions = store / "sessions"
        sessions.mkdir(parents=True)
        data = root / "data"
        (data / "inbox").mkdir(parents=True)
        env = {**os.environ, "CODE_SESSION_INCLUDE_DEFAULTS": "0",
               "CODE_SESSION_ROOTS": str(store), "CODE_SESSION_DATA_DIR": str(data)}
        work = {"code_home": str(store), "cwd": str(root),
                "policy": {"mode": "danger-full-access"}}

        def call(request, policy=None):
            payload = {**work, "request": request}
            if policy is not None:
                payload["policy"] = policy
            return invoke(artifact, json.dumps(payload).encode(), env)

        source = sessions / "rollout-test.jsonl"
        source.write_text("\n".join(json.dumps(record) for record in [
            {"type": "session_meta", "payload": {"id": "fixture", "cwd": str(root)}},
            {"type": "response_item", "payload": {"type": "message", "role": "user", "content": "bounded history needle"}},
        ]) + "\n", encoding="utf-8")
        stores = call({"action": "list_stores"})
        assert stores["ok"] is True, stores
        assert stores["count"] == 1, stores
        found = call({"action": "search_messages", "query": "needle"})
        assert found["ok"] is True and found["count"] == 1, found
        malformed = invoke(artifact, b"{", env)
        assert malformed["error"]["code"] == "invalid_argument", malformed
        oversized = invoke(artifact, b" " * (1024 * 1024 + 1), env)
        assert oversized["error"]["code"] == "limit_exceeded", oversized
        denied = call({"action": "import_openai_export", "file_name": "export.json",
                       "dataset_name": "denied"}, {"mode": "read-only"})
        assert denied["error"]["code"] == "permission_denied", denied
        assert not (data / "datasets/openai-denied").exists()
        assert not list(data.glob("datasets/.import-*"))

        # Valid JSON smaller than the import byte cap, but with enough tiny
        # values to exceed the worker's allocation budget when decoded.
        heavy = data / "inbox/heavy.json"
        conversation = {"id": "heavy", "current_node": "u", "mapping": {
            "u": {"id": "u", "parent": None, "message": {
                "author": {"role": "user"},
                "content": {"content_type": "text", "parts": ["bounded"]}}}}}
        with heavy.open("wb") as stream:
            stream.write(b"[" + json.dumps(conversation).encode()[:-1] + b',"unused":[')
            for _ in range(4096):
                stream.write(b"0," * 4096)
            stream.write(b"0]}]")
        original_size = heavy.stat().st_size
        result = run_worker(artifact, json.dumps({**work, "request": {
            "action": "import_openai_export", "file_name": "heavy.json",
            "dataset_name": "memory-pressure"}}).encode(), env)
        assert result.returncode != 0 or json.loads(result.stdout)["ok"] is False
        assert not (data / "datasets/openai-memory-pressure").exists()
        assert heavy.stat().st_size == original_size
        assert call({"action": "list_stores"})["ok"] is True
    print("Native history worker: retrieval, malformed/oversized input, write policy, and memory-pressure isolation passed")


if __name__ == "__main__":
    main()
