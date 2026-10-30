# Rustwright MCP source snapshot

The files under `src/` and the tool-catalog test fixture are adapted from the
native MCP server in `Skyvern-AI/rustwright` at commit
`7d8f4c092cfa3f26d5307b5385233160e70e86a2` (October 15, 2026).

Code carries this source snapshot because the upstream `rustwright-mcp` package
currently exposes only a binary target. The local `lib.rs` supplies an embedded
stdio entry point so the Code executable can host the server without a separate
runtime installer or sidecar binary. Keep behavior changes in sync with the
upstream `mcp/` directory and retain the accompanying MIT license.

The adapter uses Code's workspace `serde`, `serde_json`, and `tokio` versions
instead of the standalone server's exact pins so it can share Code's dependency
graph. Rustwright's engine and agent source revision remains fixed above.
