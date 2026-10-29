# SSH connection registry

`/ssh` manages saved PCs without a hardcoded machine list. No connections are
pre-registered. The model-facing `ssh` tool can execute bounded commands through
these registered profiles; SCP project folders remain separate work.

## Settings menu

Open `/settings ssh`, or choose **SSH** in the settings overview. The same
registry view works in the full overlay and the compact bottom pane.

- **N** adds a PC; **E** or Enter edits its name and endpoints.
- **P** configures password authentication; **K** configures a key or SSH agent.
- **D** opens a removal confirmation; **R** reloads the saved registry.
- Use Up/Down or the mouse wheel to select a PC. Click a row to select it.
- Forms use Tab to move fields and Enter or Ctrl+S to submit. Escape cancels.

An empty registry explicitly shows **No PCs registered**. Malformed-registry
errors are shown rather than silently displaying an empty list. Edits and
removal confirmations reject changes made against stale profile snapshots.
Both slash-command and menu results are local notices, not assistant-stream
updates; they must not replace or disappear behind an existing model answer.

## Commands

- `/ssh` or `/ssh list`: list all registered PCs in case-insensitive name order.
- `/ssh add user@host,user@alternate pwd "Office PC"`: register ordered endpoints
  and open a masked local password form.
- `/ssh add user@host key "Office PC"`: open the key form. Enter a local private-key
  path, or leave it blank to select SSH-agent authentication. The passphrase field
  is optional for key files; it must be blank when using an agent.
- `/ssh show "Office PC"`: show the connection ID, configured authentication
  methods, and endpoint attempt order, without reading credential values.
- `/ssh update "Office PC" user@new-host,user@alternate`: change endpoints while
  retaining the connection ID and credentials.
- `/ssh rename "Office PC" "Lab PC"`: change its name without changing its ID.
- `/ssh auth "Lab PC" pwd`: add or replace password authentication and make it
  the default. Use `key` to configure key authentication. The other configured
  authentication method is retained.
- `/ssh remove "Lab PC"`: remove the profile and its saved SSH credentials.

Quote names containing spaces. Duplicate names are rejected case-insensitively.
`self`, `127.0.0.1`, and `0.0.0.0` are reserved for the local-execution aliases
and cannot be registered as PC names.

Endpoints accept `user@host`, `user@host:port`, and `ssh://user@host:port`.
Bracket IPv6 addresses: `user@[2001:db8::1]:2222`. Inline passwords, remote
paths, URL queries/fragments, invalid ports, and duplicate endpoints are rejected.

## Endpoint ordering

The registry preserves the entered order. Its attempt-order view first returns
endpoints outside `100.64.0.0/10`, then those inside that range, preserving
left-to-right order within both groups. Both `100.64.0.0` and `100.127.255.255`
are included; `100.63.*` and `100.128.*` are not. IPv4-mapped IPv6 addresses use
the same rule. This is an ordering preference, not proof of Tailscale membership
or permission to trust a host.

Registration performs no DNS lookups or connectivity tests. Hostnames retain
their entered order in the first group; the transport resolves and verifies
each endpoint only when a command is executed.

## Credentials and persistence

Connection metadata is stored in `CODE_HOME/ssh-connections.json`. The registry
uses a versioned map, private atomic writes, and an inter-process lock so concurrent
sessions do not silently overwrite each other's additions. It has no fixed PC
count; the serialized metadata has a 4 MiB budget. Malformed or unsupported
registries fail explicitly rather than being replaced with an empty registry.

Passwords and key passphrases are stored in the OS credential store under
`Code SSH`; metadata contains only opaque credential IDs and local key-file
paths. If secure storage is unavailable, password/passphrase registration fails
without a plaintext fallback. Agent or unencrypted-key registration does not
require storing a secret. Native Linux builds use the persistent Secret Service
backend. On Linux, the portable APE uses Linux-gated kernel keyring syscalls
without depending on APE libc support, external helpers, or a desktop D-Bus
service. Credentials remain available to Code processes owned by the same user
and must be re-entered after the login session or machine restarts. On non-Linux
APE hosts, password and passphrase storage remains unavailable until a
host-native secure-store adapter exists.

The credential form masks secret values and suppresses raw key/paste session
diagnostics while open. Enter or Ctrl+S submits; Escape cancels without saving.
An authentication form cannot overwrite a profile changed by another session
after the form opened. Closing or renaming a profile never connects to its host.

Registration does not accept or verify host keys. Remote execution requires an
existing known host key because the transport uses strict host-key checking; it
does not accept a first-use or changed key automatically. If credential cleanup
fails after a removal/update, the error explicitly reports the already-applied
metadata change and retained credential.

## Agent-facing command execution

The stable model-facing function name is `ssh`; profile names are arguments, not
dynamic tool names. A command call has this shape:

```json
{"name":"Office PC","command":"uname -a"}
```

`auth` may select an already-configured `pwd` or `key` method; omitting it uses
the profile default. `timeout_ms` is optional and is clamped to one second
through thirty minutes, with a ten-minute default. The command is passed to SSH
as one remote-command argument, so Code does not rewrite its quoting or install
local shell aliases. Endpoint alternatives are tried in the registry's ordered
failover order only for connection failures; authentication and host-key errors
are not retried.

The structured shell tool also accepts the same dispatcher through
`shell ssh <name> [pwd|key] [--] <command>`. Use `--` when the first command word
is `pwd` or `key`, for example `shell ssh Office-PC -- pwd`. The reserved names
`self`, `127.0.0.1`, and `0.0.0.0` execute through the existing local shell backend
instead of opening an SSH connection. SCP upload/download and remote project
folders are not part of this command-execution slice.
