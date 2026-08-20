# MCP Server Reload Design

<!-- markdownlint-disable MD013 -->

## Goal

Allow a running Code session to restart one configured MCP server or every
configured MCP server without restarting Code. Reload must replace dead stdio
children, reconnect HTTP clients, and rebuild the active tool/resource status.

## User Interface

- `/mcp reload <name>` reloads one configured server.
- `/mcp reload all` reloads every configured server.
- The MCP settings refresh action reloads all servers instead of only invoking
  `tools/list` on existing clients.
- A reload request is acknowledged immediately. The existing MCP status model
  reports the resulting tools or startup failure.
- `code mcp list/add/remove` remains configuration management and MUST NOT be
  presented as a live-session restart mechanism.

## Runtime Design

Add an active-core protocol operation carrying an optional server name. The
submission loop resolves names against the current session configuration and
calls a manager-level reload method.

For each server, the connection manager:

1. Removes the current client from the live client map.
2. Removes stale tools by refreshing inventory without that client.
3. Shuts down the removed client outside the map lock.
4. Starts a new client from the current `McpServerConfig`.
5. Refreshes tools, resources, authentication state, and failures through the
   existing status response path.

If startup fails, the dead client remains removed, stale tools remain absent,
and the failure is stored under that server name. Reloading an unknown or
disabled server returns a visible error rather than silently doing nothing.

Disabling and re-enabling a configured server from the TUI MUST update both the
persisted configuration and the live connection state. Disabling a server MUST
shut down its active client and remove its tools. Re-enabling it MUST start a
new client from the current configuration.

## App Server

The existing `config/mcpServer/reload` request MUST perform a real reload. It
MUST load the effective configuration, propagate a reload request to every
loaded thread, and return only after the requests have been accepted. The
current behavior that loads configuration and returns an empty response without
updating live threads is not sufficient.

Reload requested through the app server MAY be applied at the next safe turn
boundary, but stale clients MUST NOT remain selectable after the reload has
been accepted.

## Scope

The feature reuses the current in-memory configuration. It does not add a
daemon, signal protocol, configuration watcher, automatic retry loop, or new
CLI process-management command. `code mcp list/add/remove` remains configuration
management; reload is an active-session operation.

## Verification

- Unit tests cover removal of stale state and failed restart reporting.
- TUI command tests cover named reload, `all`, missing arguments, and unknown
  subcommand help text.
- Settings tests verify the refresh action submits the reload operation.
- App-server tests verify `config/mcpServer/reload` updates loaded threads rather
  than acting as a no-op.
- Toggle tests independently verify both disable-to-stop and enable-to-start
  directions.
- Single-threaded Rust tests and build checks run for each affected crate.
- A separately launched Code process verifies that reloading a local stdio MCP
  server changes its process ID and restores its tool list.
