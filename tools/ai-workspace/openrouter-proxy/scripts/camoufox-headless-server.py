from __future__ import annotations

import os

from camoufox.server import launch_server


def main() -> int:
    port = int(os.environ.get("CAMOUFOX_PORT", "8765"))
    ws_path = os.environ.get("CAMOUFOX_WS_PATH", "openrouter-proxy")

    print(f"Starting Camoufox headless server on ws://127.0.0.1:{port}/{ws_path}", flush=True)
    launch_server(
        headless=True,
        geoip=False,
        port=port,
        ws_path=ws_path,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
