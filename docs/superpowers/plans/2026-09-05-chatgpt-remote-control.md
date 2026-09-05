# ChatGPT Remote-Control Service Implementation Plan

<!-- markdownlint-disable MD013 -->

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the OpenAI Codex 0.153.4 ChatGPT remote-control service to
code-mod's server-side runtime with persistent daemon operation, pairing,
client management, and official relay compatibility.

**Architecture:** Add the upstream remote-control protocol and relay behavior
as focused modules inside the existing `code-app-server`, so remote devices
feed the same transport events and JSON-RPC processor as stdio and local
WebSocket clients. Add a narrowly scoped SQLite enrollment store and a small
daemon crate rather than importing the newer general Codex state, transport,
configuration, and HTTP-client architecture.

**Tech Stack:** Rust 2024, Tokio, tokio-tungstenite, Reqwest, SQLx/SQLite,
Serde, Schemars, ts-rs, Clap, Unix-domain sockets, local HTTP/WebSocket test
fixtures, and the repository `build-fast.sh` verifier.

---

## Constraints

- Preserve all pre-existing operator-owned changes in the dirty worktree.
- Keep implementation changes under `code-rs`; use the local OpenAI Codex
  `rust-v0.153.4` checkout only as a behavioral source.
- Use `CARGO_BUILD_JOBS=1` for every Rust build, check, or test command.
- Never run rustfmt.
- Treat compiler warnings as failures.
- Do not contact production ChatGPT services in automated tests.
- Do not expose or launch a gameplay application during verification.
- Commit only files belonging to the task being completed.

## Upstream Reference Map

- `/var/home/jack/.code-tmp/openai-codex-0.153.4/src/codex-rs/app-server-protocol/src/protocol/v2/remote_control.rs`
- `/var/home/jack/.code-tmp/openai-codex-0.153.4/src/codex-rs/app-server/src/request_processors/remote_control_processor.rs`
- `/var/home/jack/.code-tmp/openai-codex-0.153.4/src/codex-rs/app-server-transport/src/transport/remote_control/`
- `/var/home/jack/.code-tmp/openai-codex-0.153.4/src/codex-rs/app-server-daemon/`
- `/var/home/jack/.code-tmp/openai-codex-0.153.4/src/codex-rs/state/src/runtime/remote_control.rs`
- `/var/home/jack/.code-tmp/openai-codex-0.153.4/src/codex-rs/cli/src/remote_control_cmd.rs`

## File Map

- `code-rs/Cargo.toml`: workspace members and shared dependencies.
- `code-rs/Cargo.lock`: resolved dependency graph.
- `code-rs/app-server-protocol/src/protocol/remote_control.rs`: typed public
  remote-control API.
- `code-rs/app-server-protocol/src/protocol/common.rs`: request and notification
  wire-method registration.
- `code-rs/app-server-protocol/src/protocol/mod.rs`: protocol module export.
- `code-rs/app-server-protocol/src/lib.rs`: crate-level re-export.
- `code-rs/app-server-protocol/src/export.rs`: schema inclusion assertions.
- `code-rs/app-server-protocol/schema/`: regenerated JSON and TypeScript
  protocol fixtures.
- `code-rs/app-server/src/remote_control/auth.rs`: active ChatGPT auth loading
  and coordinated unauthorized recovery.
- `code-rs/app-server/src/remote_control/protocol.rs`: hosted relay event model,
  endpoint normalization, and wire parsing.
- `code-rs/app-server/src/remote_control/segment.rs`: bounded segmentation and
  reassembly.
- `code-rs/app-server/src/remote_control/state.rs`: SQLite enrollment store.
- `code-rs/app-server/src/remote_control/server_api.rs`: enroll, refresh,
  pairing, and pairing-status HTTP operations.
- `code-rs/app-server/src/remote_control/clients.rs`: list and revoke client HTTP
  operations.
- `code-rs/app-server/src/remote_control/host_device.rs`: host identity sent to
  ChatGPT.
- `code-rs/app-server/src/remote_control/desired_state.rs`: serialized
  persistent and ephemeral state transitions.
- `code-rs/app-server/src/remote_control/enroll.rs`: enrollment selection,
  refresh, replacement, and persistence.
- `code-rs/app-server/src/remote_control/websocket.rs`: outbound relay,
  reconnects, and stream multiplexing.
- `code-rs/app-server/src/remote_control/client_tracker.rs`: per-client stream
  lifecycle and ordered writes.
- `code-rs/app-server/src/remote_control/mod.rs`: public handle and task
  orchestration.
- `code-rs/core/src/auth.rs`: auth-change notifications and coordinated
  unauthorized recovery used by the relay.
- `code-rs/app-server/src/remote_control_processor.rs`: v2 JSON-RPC request
  adapter.
- `code-rs/app-server/src/transport.rs`: Unix socket, transport-off, and remote
  connection integration.
- `code-rs/app-server/src/lib.rs`: runtime options, startup, status broadcast,
  and shutdown coordination.
- `code-rs/app-server/src/message_processor.rs`: remote-control request
  dispatch.
- `code-rs/app-server/Cargo.toml`: relay and persistence dependencies.
- `code-rs/app-server-daemon/Cargo.toml`: new daemon crate manifest.
- `code-rs/app-server-daemon/src/`: lifecycle, socket client, settings, and
  remote-control controller.
- `code-rs/cli/src/remote_control_cmd.rs`: foreground and daemon command UX.
- `code-rs/cli/src/main.rs`: command registration and daemon dispatch.
- `code-rs/cli/src/lib.rs`: command module export.
- `code-rs/cli/Cargo.toml`: daemon dependency.

### Task 1: Add the Public Remote-Control Protocol

**Requirements:** Exact 0.153.4 wire names, camel-case fields, nullable params,
status values, pairing validation inputs, client pagination, and status
notification.

**Files:**

- Create: `code-rs/app-server-protocol/src/protocol/remote_control.rs`
- Modify: `code-rs/app-server-protocol/src/protocol/mod.rs`
- Modify: `code-rs/app-server-protocol/src/lib.rs`
- Modify: `code-rs/app-server-protocol/src/protocol/common.rs`
- Modify: `code-rs/app-server-protocol/src/export.rs`
- Create: `code-rs/app-server-protocol/tests/remote_control.rs`

- [ ] **Step 1: Write failing serialization tests**

Add tests that deserialize and reserialize every request method and the status
notification. Include the nullable enable/disable params contract and these
status values:

```rust
assert_eq!(
    serde_json::to_value(RemoteControlConnectionStatus::Disabled)?,
    serde_json::json!("disabled")
);
assert_eq!(
    serde_json::to_value(RemoteControlConnectionStatus::Errored)?,
    serde_json::json!("errored")
);
```

- [ ] **Step 2: Run the tests and verify the missing-type failure**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server-protocol --test remote_control`

Expected: compilation fails because the remote-control types and enum variants
do not exist.

- [ ] **Step 3: Add the official typed contract**

Port the 0.153.4 structs and enums into `protocol/remote_control.rs`, preserving
the upstream derives and TypeScript export location. Register these methods in
`client_request_definitions!`:

```rust
#[experimental("remoteControl/enable")]
RemoteControlEnable => "remoteControl/enable" {
    params: #[serde(skip_serializing_if = "Option::is_none")]
        remote_control::NullableRemoteControlEnableParams,
    response: remote_control::RemoteControlEnableResponse,
},
#[experimental("remoteControl/disable")]
RemoteControlDisable => "remoteControl/disable" {
    params: #[serde(skip_serializing_if = "Option::is_none")]
        remote_control::NullableRemoteControlDisableParams,
    response: remote_control::RemoteControlDisableResponse,
},
#[experimental("remoteControl/status/read")]
RemoteControlStatusRead => "remoteControl/status/read" {
    params: #[ts(type = "undefined")] #[serde(skip_serializing_if = "Option::is_none")] Option<()>,
    response: remote_control::RemoteControlStatusReadResponse,
},
```

Register the remaining request methods explicitly:

```rust
#[experimental("remoteControl/pairing/start")]
RemoteControlPairingStart => "remoteControl/pairing/start" {
    params: remote_control::RemoteControlPairingStartParams,
    response: remote_control::RemoteControlPairingStartResponse,
},
#[experimental("remoteControl/pairing/status")]
RemoteControlPairingStatus => "remoteControl/pairing/status" {
    params: remote_control::RemoteControlPairingStatusParams,
    response: remote_control::RemoteControlPairingStatusResponse,
},
#[experimental("remoteControl/client/list")]
RemoteControlClientsList => "remoteControl/client/list" {
    params: remote_control::RemoteControlClientsListParams,
    response: remote_control::RemoteControlClientsListResponse,
},
#[experimental("remoteControl/client/revoke")]
RemoteControlClientsRevoke => "remoteControl/client/revoke" {
    params: remote_control::RemoteControlClientsRevokeParams,
    response: remote_control::RemoteControlClientsRevokeResponse,
},
```

Then register the server notification:

```rust
RemoteControlStatusChanged => "remoteControl/status/changed"
    (remote_control::RemoteControlStatusChangedNotification),
```

- [ ] **Step 4: Run protocol tests**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server-protocol remote_control`

Expected: all remote-control protocol tests pass.

- [ ] **Step 5: Commit the protocol contract**

Run: `git add code-rs/app-server-protocol && git commit -m 'feat(app-server): add remote control protocol'`

### Task 2: Regenerate and Verify Protocol Schemas

**Requirements:** Generated JSON Schema and TypeScript fixtures must match the
typed API and contain no hand-edited drift.

**Files:**

- Modify: `code-rs/app-server-protocol/schema/json/`
- Modify: `code-rs/app-server-protocol/schema/typescript/`
- Modify: `code-rs/app-server-protocol/src/schema_fixtures.rs`
- Modify: `code-rs/app-server-protocol/src/export.rs`

- [ ] **Step 1: Add schema presence assertions**

Assert that the fixture tree contains the remote-control request, response,
notification, client, and enum outputs and that the flat v2 bundle exposes the
same types.

```rust
assert!(fixture_tree.contains_key(Path::new(
    "v2/RemoteControlEnableResponse.ts"
)));
assert!(fixture_tree.contains_key(Path::new(
    "v2/RemoteControlStatusChangedNotification.ts"
)));
```

- [ ] **Step 2: Run schema tests and verify stale fixtures fail**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server-protocol schema_fixture`

Expected: failure reports missing or stale generated remote-control files.

- [ ] **Step 3: Regenerate fixtures through the repository generator**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo run -p code-app-server-protocol --bin write_schema_fixtures -- --experimental`

Do not edit generated files manually.

- [ ] **Step 4: Verify generated output**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server-protocol schema_fixture`

Expected: schema fixture and export tests pass.

- [ ] **Step 5: Commit schemas**

Run: `git add code-rs/app-server-protocol && git commit -m 'chore(app-server): generate remote control schemas'`

### Task 3: Add the Enrollment Store

**Requirements:** Idempotent SQLite initialization, account isolation,
client-name isolation, persisted enabled state, no persisted secrets, and
targeted record deletion.

**Files:**

- Create: `code-rs/app-server/src/remote_control/state.rs`
- Create: `code-rs/app-server/src/remote_control/state_tests.rs`
- Modify: `code-rs/app-server/Cargo.toml`
- Modify: `code-rs/Cargo.toml`
- Modify: `code-rs/Cargo.lock`

- [ ] **Step 1: Write failing store tests**

Cover insert, update, lookup, enabled-state update, and deletion using a
temporary code home. Verify that the key is the tuple
`(websocket_url, account_id, app_server_client_name)` and inspect the table
columns to prove token fields are absent.

```rust
assert_eq!(
    stored,
    Some(RemoteControlEnrollmentRecord {
        websocket_url: target.websocket_url.clone(),
        account_id: "account-a".to_string(),
        app_server_client_name: Some("desktop".to_string()),
        server_id: "srv_a".to_string(),
        environment_id: "env_a".to_string(),
        server_name: "deck".to_string(),
        remote_control_enabled: Some(true),
    })
);
```

- [ ] **Step 2: Run the store tests and verify failure**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server remote_control::state_tests`

Expected: compilation fails because the store does not exist.

- [ ] **Step 3: Implement the narrow SQLite store**

Create `RemoteControlState` with an internal `SqlitePool` and these operations:

```rust
pub async fn open(code_home: &Path) -> anyhow::Result<Arc<Self>>;
pub async fn get_enrollment(
    &self,
    websocket_url: &str,
    account_id: &str,
    app_server_client_name: Option<&str>,
) -> anyhow::Result<Option<RemoteControlEnrollmentRecord>>;
pub async fn upsert_enrollment(
    &self,
    enrollment: &RemoteControlEnrollmentRecord,
) -> anyhow::Result<()>;
pub async fn set_enabled(
    &self,
    websocket_url: &str,
    account_id: &str,
    app_server_client_name: Option<&str>,
    enabled: bool,
) -> anyhow::Result<u64>;
pub async fn delete_enrollment(
    &self,
    websocket_url: &str,
    account_id: &str,
    app_server_client_name: Option<&str>,
) -> anyhow::Result<u64>;
```

Use WAL mode, normal synchronous mode, a one-connection pool for serialized
schema setup, and an idempotent `CREATE TABLE IF NOT EXISTS`. Store an empty
string for a missing client name to preserve the upstream composite primary
key behavior.

- [ ] **Step 4: Run the store tests**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server remote_control::state_tests`

Expected: all persistence tests pass.

- [ ] **Step 5: Commit persistence**

Run: `git add code-rs/Cargo.toml code-rs/Cargo.lock code-rs/app-server && git commit -m 'feat(app-server): persist remote control enrollment'`

### Task 4: Implement Endpoint Normalization and Auth Recovery

**Requirements:** Production-host allowlist, localhost test support,
ChatGPT-only authentication, account header injection, and one coordinated
unauthorized recovery attempt.

**Files:**

- Create: `code-rs/app-server/src/remote_control/protocol.rs`
- Create: `code-rs/app-server/src/remote_control/auth.rs`
- Create: `code-rs/app-server/src/remote_control/protocol_tests.rs`
- Create: `code-rs/app-server/src/remote_control/auth_tests.rs`
- Modify: `code-rs/app-server/Cargo.toml`
- Modify: `code-rs/core/src/auth.rs`

- [ ] **Step 1: Write failing URL normalization tests**

Cover `https://chatgpt.com/backend-api`, staging subdomains, localhost HTTP and
HTTPS, pre-normalized paths, trailing slashes, and rejection of lookalike or
insecure production hosts.

```rust
assert_eq!(
    normalize_remote_control_url("https://chatgpt.com/backend-api")?.websocket_url,
    "wss://chatgpt.com/backend-api/wham/remote/control/server"
);
assert!(normalize_remote_control_url("https://chatgpt.com.evil.test/backend-api").is_err());
```

- [ ] **Step 2: Write failing auth tests**

Use a test auth provider to verify bearer-token and `ChatGPT-Account-ID`
headers, rejection of API-key/local-provider auth, auth reload after a missing
account ID, and a single recovery attempt across concurrent unauthorized
operations.

- [ ] **Step 3: Run tests and verify failure**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server remote_control::`

Expected: compilation fails because normalization and auth adapters do not
exist.

- [ ] **Step 4: Implement adapters over code-mod auth**

Extend the existing `AuthManager` with the upstream-compatible auth revision
watch and one-at-a-time unauthorized recovery helper. The remote-control module
then uses a small testable adapter instead of duplicating account storage:

```rust
#[async_trait::async_trait]
pub(crate) trait RemoteControlAuthProvider: Send + Sync {
    async fn load(&self) -> io::Result<RemoteControlAuth>;
    async fn recover_unauthorized(&self) -> io::Result<bool>;
    fn subscribe(&self) -> watch::Receiver<u64>;
}
```

Increment the auth revision only when the account or request credentials
change. The production adapter must read the active account through existing
`code-core` authentication/account-selection APIs and delegate token recovery
to `AuthManager::refresh_token_classified`. `RemoteControlAuth` holds only
in-memory request credentials and exposes a redaction-safe header builder.

- [ ] **Step 5: Run normalization and auth tests**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server remote_control::`

Expected: all tests pass with no credential values in assertion failure text.

- [ ] **Step 6: Commit auth and normalization**

Run: `git add code-rs/app-server code-rs/core/src/auth.rs code-rs/Cargo.toml code-rs/Cargo.lock && git commit -m 'feat(app-server): add remote control auth and endpoints'`

### Task 5: Implement Enrollment and Client-Management HTTP APIs

**Requirements:** Enroll, refresh, pair, pairing status, list clients, revoke
client, bounded response previews, typed status mapping, and auth recovery.

**Files:**

- Create: `code-rs/app-server/src/remote_control/server_api.rs`
- Create: `code-rs/app-server/src/remote_control/server_api_tests.rs`
- Create: `code-rs/app-server/src/remote_control/clients.rs`
- Create: `code-rs/app-server/src/remote_control/clients_tests.rs`
- Create: `code-rs/app-server/src/remote_control/host_device.rs`
- Create: `code-rs/app-server/src/remote_control/enroll.rs`
- Create: `code-rs/app-server/src/remote_control/enroll_tests.rs`

- [ ] **Step 1: Write failing local HTTP fixture tests**

Use a local listener to assert exact methods, endpoint paths, JSON bodies,
authorization headers, account headers, timeout behavior, and response parsing.
Cover 401/403 auth recovery, 404 stale enrollment, malformed JSON, and bounded
body previews that redact token-shaped fields.

- [ ] **Step 2: Write failing enrollment-selection tests**

Verify `ReuseOrCreate` loads the matching persisted server, `ReplaceExisting`
enrolls a new identity, account changes cannot reuse an old record, refresh
recovers a short-lived server token without persisting it, and failed refresh
falls back to replacement enrollment only for stale/invalid identities.

- [ ] **Step 3: Run tests and verify failure**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server remote_control::`

Expected: compilation fails because the HTTP and enrollment modules do not
exist.

- [ ] **Step 4: Implement the HTTP client functions**

Expose narrowly scoped operations with `io::Result` boundaries:

```rust
pub(crate) async fn enroll_remote_control_server(
    target: &RemoteControlTarget,
    auth: &RemoteControlAuth,
    installation_id: &str,
    host: &HostDevice,
) -> io::Result<RemoteControlEnrollment>;

pub(crate) async fn refresh_remote_control_server(
    enrollment: &RemoteControlEnrollment,
    auth: &RemoteControlAuth,
) -> io::Result<RemoteControlEnrollment>;
```

Pairing must use the short-lived server token; client list/revoke must use the
active account bearer token. Never include raw response bodies beyond the
bounded redacted preview helper.

- [ ] **Step 5: Run HTTP and enrollment tests**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server remote_control::`

Expected: all local-fixture tests pass.

- [ ] **Step 6: Commit the server API layer**

Run: `git add code-rs/app-server && git commit -m 'feat(app-server): add remote control enrollment APIs'`

### Task 6: Implement Relay Framing and Segmentation

**Requirements:** Exact hosted relay event representation, bounded 100 KiB
target segmentation, ordered reassembly, duplicate detection, and malformed
sequence rejection.

**Files:**

- Modify: `code-rs/app-server/src/remote_control/protocol.rs`
- Create: `code-rs/app-server/src/remote_control/segment.rs`
- Create: `code-rs/app-server/src/remote_control/segment_tests.rs`

- [ ] **Step 1: Port protocol fixtures into failing tests**

Cover server ready, client connected/disconnected, message, message segment,
ping/pong, protocol error, stream IDs, client IDs, and sequence metadata using
the exact upstream JSON fixtures.

- [ ] **Step 2: Add segmentation property cases**

Verify empty, one-byte, boundary-sized, multi-segment, Unicode, and maximum
accepted payloads. Test missing first segment, duplicate index, out-of-order
index, changed segment count, unknown client, and over-limit accumulation.

```rust
assert_eq!(REMOTE_CONTROL_SEGMENT_TARGET_BYTES, 100 * 1024);
assert_eq!(reassemble(segment(payload.clone()))?, payload);
```

- [ ] **Step 3: Run tests and verify failure**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server remote_control::segment_tests`

Expected: framing and segmentation symbols are missing.

- [ ] **Step 4: Implement bounded framing**

Port the official 0.153.4 event enums and segmentation algorithm. Keep a hard
upper bound on concurrent assemblies and total buffered bytes. Completing or
rejecting an assembly must release its buffer immediately.

- [ ] **Step 5: Run framing tests**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server remote_control::segment_tests`

Expected: all framing, round-trip, and malformed-input tests pass.

- [ ] **Step 6: Commit framing**

Run: `git add code-rs/app-server/src/remote_control && git commit -m 'feat(app-server): add remote relay framing'`

### Task 7: Implement the Outbound Relay and Client Multiplexing

**Requirements:** Outbound WebSocket connection, one app-server connection per
remote client stream, ordered writes, bidirectional routing, reconnects,
re-enrollment, auth-change handling, and clean shutdown.

**Files:**

- Create: `code-rs/app-server/src/remote_control/client_tracker.rs`
- Create: `code-rs/app-server/src/remote_control/client_tracker_tests.rs`
- Create: `code-rs/app-server/src/remote_control/websocket.rs`
- Create: `code-rs/app-server/src/remote_control/websocket_tests.rs`
- Modify: `code-rs/app-server/src/transport.rs`
- Modify: `code-rs/app-server/src/outgoing_message.rs`

- [ ] **Step 1: Write failing remote-to-server routing tests**

Start a local relay fixture, open two remote clients, send independent
initialize and request messages, and assert distinct `ConnectionId` values and
correct `TransportEvent::IncomingMessage` delivery. Close one client and prove
the other remains active.

- [ ] **Step 2: Write failing server-to-remote routing tests**

Send direct responses and broadcasts through `OutgoingEnvelope`; assert direct
responses reach only the owning stream and broadcasts reach every initialized
remote stream exactly once. Exercise segmented outgoing messages separately.

- [ ] **Step 3: Write failing recovery tests**

Cover transient disconnect/backoff, unauthorized reconnect with one auth
recovery, expired server token refresh, stale enrollment replacement, account
change reconnect, duplicate stream open, queue saturation, and cancellation.

- [ ] **Step 4: Run tests and verify failure**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server remote_control::`

Expected: relay and client tracker types are missing.

- [ ] **Step 5: Implement client tracking and relay loop**

Use the existing bounded `TransportEvent` channel. Allocate connection IDs from
the same atomic sequence as local transports. Each remote stream stores its
client ID, stream ID, writer queue, initialization state, and close notifier.
The relay task owns WebSocket I/O and reports lifecycle changes through the
normal transport event path.

```rust
pub(crate) struct RemoteControlChannels {
    pub(crate) transport_event_tx: mpsc::Sender<TransportEvent>,
    pub(crate) outgoing_rx: mpsc::Receiver<QueuedRemoteMessage>,
}
```

Use cancellation-aware backoff and do not reconnect after explicit disablement
or process shutdown.

- [ ] **Step 6: Run relay tests**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server remote_control::`

Expected: bidirectional, isolation, recovery, and shutdown tests pass.

- [ ] **Step 7: Commit relay transport**

Run: `git add code-rs/app-server && git commit -m 'feat(app-server): multiplex remote control clients'`

### Task 8: Add the Remote-Control Handle and State Machine

**Requirements:** Persistent and ephemeral enable/disable, serialized
transitions, pairing, client management, status watch channel, policy checks,
and no cross-account enrollment races.

**Files:**

- Create: `code-rs/app-server/src/remote_control/desired_state.rs`
- Create: `code-rs/app-server/src/remote_control/mod.rs`
- Create: `code-rs/app-server/src/remote_control/tests.rs`
- Modify: `code-rs/app-server/src/lib.rs`

- [ ] **Step 1: Write failing state-machine tests**

Cover startup disabled, persisted enabled resolution, persistent enable,
persistent disable, ephemeral enable, ephemeral disable, status transitions,
pairing serialization, concurrent enable calls, account changes during
enrollment, and shutdown while connecting.

- [ ] **Step 2: Run tests and verify failure**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server remote_control::tests`

Expected: `RemoteControlHandle` and startup orchestration are missing.

- [ ] **Step 3: Implement the handle**

Expose the same core operations as upstream:

```rust
pub(crate) fn status(&self) -> RemoteControlStatusChangedNotification;
pub(crate) async fn resolve_persisted_preference(
    &self,
    app_server_client_name: Option<&str>,
) -> io::Result<bool>;
pub(crate) async fn enable(
    &self,
    app_server_client_name: Option<&str>,
) -> io::Result<RemoteControlStatusChangedNotification>;
pub(crate) fn enable_ephemeral(
    &self,
) -> Result<RemoteControlStatusChangedNotification, RemoteControlEnableError>;
pub(crate) async fn disable(
    &self,
    app_server_client_name: Option<&str>,
) -> io::Result<RemoteControlStatusChangedNotification>;
pub(crate) async fn disable_ephemeral(
    &self,
) -> RemoteControlStatusChangedNotification;
```

Use one semaphore for desired-state transitions, one for persistence writes,
and one lock around the selected enrollment so pairing and reconnect cannot
replace different identities concurrently.

- [ ] **Step 4: Run state-machine tests**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server remote_control::tests`

Expected: all lifecycle tests pass.

- [ ] **Step 5: Commit orchestration**

Run: `git add code-rs/app-server && git commit -m 'feat(app-server): orchestrate remote control lifecycle'`

### Task 9: Integrate Remote Control into App-Server JSON-RPC

**Requirements:** Official v2 methods, initialization enforcement, client-name
persistence key, status notifications, shared processor path, and unchanged
local transport behavior.

**Files:**

- Create: `code-rs/app-server/src/remote_control_processor.rs`
- Create: `code-rs/app-server/src/remote_control_processor_tests.rs`
- Modify: `code-rs/app-server/src/message_processor.rs`
- Modify: `code-rs/app-server/src/lib.rs`
- Modify: `code-rs/app-server/src/transport.rs`
- Modify: `code-rs/app-server/src/main.rs`
- Modify: `code-rs/app-server/Cargo.toml`

- [ ] **Step 1: Write failing request-processor tests**

Exercise all seven methods through serialized JSON-RPC requests. Verify
pre-initialize rejection, absent-handle errors, invalid pairing-status params,
ephemeral flags, app-server client-name propagation, and error-code mapping.

- [ ] **Step 2: Write failing integrated connection tests**

Initialize one local and one remote connection, assert both can run an existing
read-only method, and assert `remoteControl/status/changed` broadcasts only to
initialized clients. Test local-to-remote and remote-to-local behavior
independently.

- [ ] **Step 3: Run tests and verify failure**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server remote_control_processor`

Expected: dispatch arms and processor are missing.

- [ ] **Step 4: Add runtime options and processor integration**

Introduce:

```rust
pub struct AppServerRuntimeOptions {
    pub remote_control_startup_mode: RemoteControlStartupMode,
    pub install_shutdown_signal_handler: bool,
}
```

Keep current public `run_main` and `run_main_with_transport` wrappers by
calling a new options-aware entry point with defaults. Construct one
`RemoteControlHandle`, pass it into `MessageProcessor`, subscribe to its status
watch channel, and enqueue status notifications through the existing outgoing
sender.

- [ ] **Step 5: Add Unix socket and transport-off modes**

Extend `AppServerTransport` with `UnixSocket { socket_path }` and `Off`. Unix
socket connections must use the same transport event lifecycle as stdio and
WebSocket connections. `Off` is valid only when remote control provides the
active connection source.

- [ ] **Step 6: Run app-server tests**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server`

Expected: all existing and new app-server tests pass.

- [ ] **Step 7: Commit app-server integration**

Run: `git add code-rs/app-server && git commit -m 'feat(app-server): expose remote control service'`

### Task 10: Add the Persistent App-Server Daemon

**Requirements:** Private control socket, lifecycle commands, readiness,
pairing controller, stable state paths, stale PID/socket recovery, and no TCP
exposure.

**Files:**

- Create: `code-rs/app-server-daemon/Cargo.toml`
- Create: `code-rs/app-server-daemon/src/lib.rs`
- Create: `code-rs/app-server-daemon/src/backend.rs`
- Create: `code-rs/app-server-daemon/src/client.rs`
- Create: `code-rs/app-server-daemon/src/remote_control_client.rs`
- Create: `code-rs/app-server-daemon/src/settings.rs`
- Create: `code-rs/app-server-daemon/src/process.rs`
- Create: `code-rs/app-server-daemon/src/tests.rs`
- Modify: `code-rs/Cargo.toml`
- Modify: `code-rs/Cargo.lock`

- [ ] **Step 1: Write failing lifecycle tests**

Use a temporary code home and a fake child backend to cover start,
already-running, readiness probe, restart, stop, not-running, version, stale
PID, stale socket, ownership/permission checks, enable, disable, and pairing.

- [ ] **Step 2: Run daemon tests and verify failure**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server-daemon`

Expected: package does not exist.

- [ ] **Step 3: Implement private state paths and locking**

Use `$CODE_HOME/app-server-daemon/` for the PID, settings, operation lock,
stable installation ID, and logs. Create directories with user-only
permissions. Before removing a stale PID or socket, verify that the recorded
process is absent and that the path is owned by the current user.

- [ ] **Step 4: Implement lifecycle and protocol client**

Start the existing `code app-server` binary with a Unix socket and remote
startup mode, wait for readiness with bounded polling, and control it through
initialized JSON-RPC calls. Do not introduce a second daemon-only wire
protocol.

- [ ] **Step 5: Run daemon tests**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-app-server-daemon`

Expected: all lifecycle, permissions, readiness, and remote-control client tests
pass.

- [ ] **Step 6: Commit daemon support**

Run: `git add code-rs/Cargo.toml code-rs/Cargo.lock code-rs/app-server-daemon && git commit -m 'feat: add app-server remote control daemon'`

### Task 11: Add CLI Commands and Machine-Readable Output

**Requirements:** Foreground mode, start, stop, pair, JSON output, clear auth
errors, signal handling, and command parser coverage.

**Files:**

- Create: `code-rs/cli/src/remote_control_cmd.rs`
- Create: `code-rs/cli/src/remote_control_cmd_tests.rs`
- Modify: `code-rs/cli/src/lib.rs`
- Modify: `code-rs/cli/src/main.rs`
- Modify: `code-rs/cli/Cargo.toml`
- Modify: `code-rs/Cargo.lock`

- [ ] **Step 1: Write failing parser tests**

Cover `code remote-control`, `start`, `stop`, `pair`, and global `--json` in
both accepted argument positions. Reject incompatible remote-provider modes and
unexpected positional arguments.

- [ ] **Step 2: Write failing output tests**

Assert stable JSON field names for daemon lifecycle, connection readiness,
pairing code, manual pairing code, environment ID, and expiry. Human output
must not print bearer tokens or server-scoped credentials.

- [ ] **Step 3: Run CLI tests and verify failure**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-cli remote_control`

Expected: command variants and handlers are missing.

- [ ] **Step 4: Implement foreground and daemon commands**

Port the upstream command shape while using code-mod names and paths. Foreground
mode creates a private temporary Unix socket, starts app-server with ephemeral
remote control, waits for connected or errored status, and exits cleanly on
signal. `start`, `stop`, and `pair` delegate to `code-app-server-daemon`.

- [ ] **Step 5: Run CLI tests**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 cargo test -p code-cli remote_control`

Expected: parser and output tests pass.

- [ ] **Step 6: Commit CLI support**

Run: `git add code-rs/cli code-rs/Cargo.lock && git commit -m 'feat(cli): add ChatGPT remote control commands'`

### Task 12: Run End-to-End Regression and Coverage Gates

**Requirements:** Complete remote-control behavior, preserved local transports,
no production network calls, no leaked secrets, and no damage to existing GPT-6
or operator changes.

**Files:**

- Create: `code-rs/app-server/tests/remote_control_e2e.rs`
- Modify tests only where current behavior requires shared fixtures.
- Inspect all changed source, manifests, schemas, and generated files.

- [ ] **Step 1: Add an end-to-end local relay test**

Run a local enroll/refresh/pair HTTP fixture and relay WebSocket fixture. Start
the real app-server runtime, connect a simulated remote client, initialize it,
invoke an existing read-only RPC, receive the response, observe a server
notification, close the stream, reconnect, and verify persisted enablement is
restored after runtime restart.

- [ ] **Step 2: Run focused package suites**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p code-app-server-protocol -p code-app-server -p code-app-server-daemon -p code-cli remote_control`

Expected: all remote-control tests pass with no warnings.

- [ ] **Step 3: Run local transport regressions**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p code-app-server transport`

Expected: stdio, local WebSocket, Unix socket, remote relay, response routing,
and disconnect cleanup tests pass.

- [ ] **Step 4: Compile all directly affected crates**

Run: `cd code-rs && CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 cargo check -p code-core -p code-app-server-protocol -p code-app-server -p code-app-server-daemon -p code-cli -p code-tui -p code-mcp-server`

Expected: command exits successfully with no warnings.

- [ ] **Step 5: Audit protocol and security coverage**

Search for all remote-control methods, endpoint strings, token fields, log
calls, `unwrap`/`expect` in production relay code, unbounded channels, and
unbounded response-body reads. Confirm every bidirectional transport feature
has independent inbound and outbound coverage.

Run: `rg -n 'remoteControl/|remote_control_token|Authorization|ChatGPT-Account-ID|unbounded_channel|\.unwrap\(|\.expect\(' code-rs/app-server code-rs/app-server-daemon code-rs/cli code-rs/app-server-protocol`

- [ ] **Step 6: Inspect the complete diff**

Compare touched files against
`/var/home/jack/.code-tmp/code-mod-gpt6-initial`. Confirm unrelated existing
changes remain byte-for-byte preserved and generated/build artifacts are not
staged.

- [ ] **Step 7: Commit end-to-end coverage**

Run: `git add code-rs/app-server/tests/remote_control_e2e.rs && git commit -m 'test: cover remote control end to end'`

- [ ] **Step 8: Continue to the Just-Every parity audit**

Do not run the repository-wide final build yet. First complete the separately
queued Just-Every Code parity audit and any approved additions, then run the
single final `build-fast.sh` gate for the combined change set.

## Risks and Mitigations

- **Large upstream subsystem:** Port by responsibility and verify each layer
  before composing the full runtime.
- **Fork architecture drift:** Feed remote clients into existing transport
  events instead of replacing the app-server processor or routing model.
- **Credential leakage:** Persist only server identity, centralize redaction,
  and test error/log rendering with token-shaped values.
- **Multi-account races:** Key persistence by account and serialize enrollment
  selection while monitoring auth changes.
- **Transport regressions:** Test stdio, local WebSocket, Unix socket, and remote
  relay directions independently.
- **Daemon safety:** Use private Unix sockets, ownership checks, bounded startup
  waits, and verified stale-state cleanup.
- **Memory pressure:** Use one Cargo worker and disable test debug info for broad
  test suites when necessary.
- **Generated schema drift:** Regenerate from typed protocol definitions and
  verify fixtures before committing.
- **Production dependency in tests:** Use local HTTP/WebSocket fixtures and
  inject auth providers; never require a real ChatGPT account.

## Success Criteria

- [ ] `code remote-control`, `start`, `stop`, `pair`, and `--json` behave as
  specified.
- [ ] The app-server implements all seven official remote-control v2 methods
  and the status notification.
- [ ] Remote clients use the same initialization, request processing,
  approvals, response routing, and notifications as local clients.
- [ ] Enrollment and enabled state persist per URL, account, and client name;
  no access, refresh, or server-scoped token is stored.
- [ ] Authentication recovery, refresh, re-enrollment, reconnect, segmentation,
  and account switching are covered by local automated tests.
- [ ] Daemon state and socket permissions are private and stale-state recovery
  is safe.
- [ ] Existing stdio and local WebSocket behavior remains unchanged when remote
  control is disabled.
- [ ] GPT-6 changes and all pre-existing operator work remain intact.
- [ ] The remote-control work passes focused tests and affected-crate checks
  before the Just-Every parity phase begins.
