# ChatGPT Remote-Control Service Design

Date: 2026-09-05
Status: Approved

## Goals

- Add the ChatGPT remote-control service introduced by OpenAI Codex 0.153.4
  to code-mod's server-side runtime.
- Preserve the official remote-control wire protocol and user-visible behavior:
  enrollment, pairing, persistent enablement, remote client management,
  reconnects, and status notifications.
- Integrate remote clients into the existing app-server JSON-RPC processing
  path so local and remote clients use the same request validation, session
  state, routing, approvals, and notifications.
- Preserve code-mod's existing stdio and local WebSocket transports,
  multi-account authentication, providers, and operator-owned changes.
- Keep the implementation focused on the remote-control subsystem instead of
  importing unrelated architectural changes from newer OpenAI Codex releases.

## Non-Goals

- Do not expose the existing unauthenticated local WebSocket listener directly
  to the internet.
- Do not implement a generic reverse proxy or emulate the ChatGPT backend.
- Do not import the complete upstream `codex-state`, `codex-api`,
  `codex-http-client`, or configuration-requirements architecture.
- Do not replace code-mod's authentication or account-selection systems.
- Do not make remote control active for a new installation without an explicit
  foreground, daemon, or app-server protocol request.
- Do not add a compile-time feature gate or retain an incomplete
  foreground-only compatibility path.

## Reference Behavior

The behavioral reference is the OpenAI Codex `rust-v0.153.4` implementation in
the local source snapshot. The relevant upstream layers are:

- `app-server-protocol`: remote-control request, response, notification, status,
  pairing, and client-management types;
- `app-server-transport`: ChatGPT enrollment APIs, outbound WebSocket relay,
  client multiplexing, message segmentation, reconnects, and auth recovery;
- `app-server`: remote-control lifecycle integration and JSON-RPC request
  processing;
- `app-server-daemon`: persistent app-server lifecycle and local control socket;
- `state`: persisted enrollment and enabled-state records;
- `cli`: foreground, start, stop, pairing, and JSON output commands.

The port will preserve those externally observable contracts while adapting
their internal dependencies to code-mod.

## Architecture

The implementation will use an integrated full port. Remote control will be a
third connection source feeding the existing `TransportEvent` and
`OutgoingEnvelope` routing pipeline alongside stdio and the local WebSocket
listener.

The subsystem will be split into five focused layers.

### Protocol Layer

`code-app-server-protocol` will define the official v2 remote-control API:

- `remoteControl/enable`;
- `remoteControl/disable`;
- `remoteControl/status/read`;
- `remoteControl/pairing/start`;
- `remoteControl/pairing/status`;
- `remoteControl/clients/list`;
- `remoteControl/clients/revoke`;
- `remoteControl/statusChanged` notifications.

The associated status, pairing, client, pagination, and response types will
retain the upstream camel-case JSON representation. Protocol serialization
tests and generated JSON Schema and TypeScript fixtures will be updated from
the typed definitions rather than edited independently.

### Relay Transport

A dedicated `code-rs/app-server/src/remote_control/` module will own the
ChatGPT relay implementation. It will:

- normalize the configured ChatGPT base URL into the official enroll, refresh,
  pair, pair-status, client-management, and WebSocket endpoints;
- enroll or refresh a server identity using the active ChatGPT account;
- connect outbound to the ChatGPT remote-control WebSocket;
- represent each remote device stream as a normal app-server connection ID;
- translate remote open, close, message, and segmented-message events into the
  existing app-server transport event channel;
- route app-server responses and broadcasts back to the correct remote client;
- reconnect with bounded backoff after transient failures;
- reload or refresh authentication after authorization failures;
- re-enroll when the stored server token or enrollment becomes invalid;
- segment oversized payloads using the upstream-compatible framing contract;
- close remote connection state cleanly during disablement or shutdown.

Remote clients will not receive a separate or reduced RPC implementation. Once
a relay stream is opened, it will pass through the same initialization and
message processor used by local clients.

### Enrollment Persistence

Because code-mod does not have the newer general-purpose `codex-state` crate,
the app-server will contain a narrowly scoped SQLite enrollment store. The
store will persist:

- normalized remote-control WebSocket URL;
- ChatGPT account ID;
- optional app-server client name;
- server ID;
- environment ID;
- server display name;
- persistent enabled or disabled preference;
- last-update timestamp.

Records will be keyed by remote-control URL, account ID, and app-server client
name so switching accounts or ChatGPT environments cannot reuse another
account's enrollment. Access tokens, refresh tokens, server-scoped
remote-control tokens, and token expiry metadata will never be persisted in
this database. A running process will recover short-lived server credentials
through the refresh endpoint using the persisted server identity.

The database will live under the code home directory in the app-server daemon
state area. Initialization and migration will be idempotent, and database
failure will disable persistent remote control with a clear error rather than
silently falling back to an untracked enrollment.

### App-Server Integration

The app-server runtime will add:

- Unix-domain-socket and transport-off modes needed by the daemon and
  foreground controller;
- remote-control startup modes for resolving a persisted preference,
  explicitly enabled ephemeral operation, and explicitly disabled ephemeral
  operation;
- a remote-control handle shared with the v2 request processor;
- status-change broadcasts to initialized app-server clients;
- shutdown coordination for the relay, local transports, and processor loop;
- a stable installation ID and host display name used during enrollment.

Persistent enable and disable requests will update the enrollment store before
publishing the desired state. Ephemeral requests will affect only the running
process. Pairing and enrollment transitions will be serialized so concurrent
requests cannot replace different server identities.

### Daemon and CLI

A new `code-app-server-daemon` crate will manage a persistent app-server over a
private Unix-domain control socket. It will provide start, restart, stop,
version, remote-control enable, remote-control disable, readiness, and pairing
operations without exposing a TCP listener.

The main CLI will add the official user-facing command shape:

- `code remote-control` for foreground remote control;
- `code remote-control start` for persistent daemon operation;
- `code remote-control stop`;
- `code remote-control pair`;
- `--json` output for automation.

The existing `code app-server` command will retain stdio as its default and
continue supporting the local WebSocket listener. Daemon control will use the
same app-server protocol methods as any other initialized client rather than a
second private lifecycle protocol.

## Authentication and Accounts

Remote control requires an active ChatGPT-backed account with an account ID.
API-key-only and local-provider sessions will fail with a clear authentication
error instead of sending incompatible credentials to ChatGPT.

The relay will use code-mod's existing authentication and multi-account
selection path. It will attach the active bearer token and
`ChatGPT-Account-ID` header to enrollment and management requests. Account
changes will invalidate the active relay identity, reload the matching
enrollment record, and reconnect under the new account.

Authentication recovery will be bounded: one coordinated reload or refresh is
attempted after an unauthorized response, after which the operation fails and
reports a disconnected status. This prevents retry loops and simultaneous
refresh storms.

## Security

- Production remote-control URLs must use HTTPS/WSS and belong to
  `chatgpt.com` or `chatgpt-staging.com`. HTTP/WS is accepted only for
  `localhost` test endpoints.
- The daemon control socket and state directory will be private to the current
  user. Stale socket and PID handling must not allow another user to redirect
  daemon control.
- Tokens, authorization headers, pairing codes, and server-scoped credentials
  will be redacted from logs and error output.
- Remote payloads must pass through the normal app-server initialization,
  capability, approval, and request-routing checks.
- Malformed relay events, invalid segment sequences, duplicate stream IDs, and
  messages for unknown clients will be rejected without terminating unrelated
  connections.
- Queue capacity and message-size handling will remain bounded to avoid remote
  clients causing unbounded memory growth.

## Lifecycle and Failure Handling

Remote-control status will use the upstream `disabled`, `connecting`,
`connected`, and `errored` protocol states.

Expected failures are handled as follows:

- unavailable or incompatible auth: remain disconnected and return a typed
  request error;
- transient HTTP or WebSocket failure: reconnect with bounded backoff;
- unauthorized enrollment or relay connection: perform one coordinated auth
  recovery and retry;
- invalid or expired enrollment: refresh, then re-enroll if refresh cannot
  recover it;
- corrupt or mismatched persisted record: discard only that record and enroll
  a replacement for the active account;
- daemon crash or stale PID/socket: verify process and socket ownership before
  cleanup, then restart safely;
- app-server shutdown: close relay clients, flush pending connection-close
  events, stop accepting new work, and release the control socket.

## Compatibility

- Existing stdio and local WebSocket app-server behavior must remain unchanged
  when remote control is disabled.
- Existing app-server clients that do not use the new methods must continue to
  initialize and operate without advertising remote-control capabilities.
- Existing code-mod account switching must remain authoritative; remote control
  must not create a parallel account registry.
- The remote-control wire format, endpoint paths, segmentation, and pairing
  semantics must remain compatible with OpenAI Codex 0.153.4.
- The implementation must not depend on the newer Responses Lite, code-mode,
  plugin, or general state-runtime subsystems that are outside this port.

## Verification

Tests will be added before production code for:

- every remote-control protocol request, response, and notification wire name;
- URL normalization and production-host restrictions;
- enrollment persistence isolation by URL, account, and client name;
- enable, disable, restart, and persisted-state resolution;
- ephemeral enablement leaving persisted state unchanged;
- enrollment, refresh, unauthorized recovery, and re-enrollment;
- pairing start and pairing-status validation;
- client listing, pagination, ordering, and revocation;
- remote client open, initialize, request, response, notification, and close
  routing through the existing app-server processor;
- bidirectional behavior for each remote stream, including independent tests
  for remote-to-server and server-to-remote delivery;
- segmented messages, malformed segments, duplicate streams, unknown clients,
  queue saturation, and reconnect behavior;
- daemon lifecycle, stale socket/PID recovery, private permissions, and JSON
  CLI output;
- unchanged stdio and local WebSocket behavior when remote control is unused.

Network integration tests will use local HTTP and WebSocket fixtures. They will
not require a real ChatGPT account or contact production services. All Rust
test and build commands will use `CARGO_BUILD_JOBS=1`, and the final repository
verification remains `CARGO_BUILD_JOBS=1 ./build-fast.sh` after the subsequent
Just-Every Code parity work is integrated.
