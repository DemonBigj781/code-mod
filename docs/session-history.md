# Native session-history

Code exposes `session_history` as a built-in Rust agent tool. It implements the
prior-sessions operations without starting Python, an MCP server, or a proxy.
The tool runs the Rust reader in a supervised child of the same executable;
history scanning does not run on a blocking thread inside the parent agent.
It reads existing Code/Codex history and the OpenAI datasets created by
`code-session-mcp`; no migration or rewrite of those source stores is required.

This is history retrieval, not conversation resumption. Use `/resume` to resume
a conversation and `/tabs restore` to reopen a saved TUI tab layout.

## Operations

Call the tool with an `action` and the relevant arguments:

- `list_stores`: discover roots, source types, warnings, and the import inbox.
- `list_sessions`: list sessions with pagination and filters.
- `search_messages`: search literal text; requires `query`.
- `get_session`: retrieve messages and optional tool records; requires
  `session_ref`.
- `get_turn`: retrieve an explicit turn; requires `session_ref` and `turn_id`.
- `inspect_store`: inspect schemas/counts without chat content; requires
  `store_id`.
- `import_openai_export`: import a new local dataset; requires `file_name` and
  `dataset_name`.

Examples of tool arguments:

- `{"action":"list_stores"}`
- `{"action":"list_sessions","store_id":"code","cwd_contains":"my-project","limit":20}`
- `{"action":"search_messages","query":"tab restoration","include_tools":false}`
- `{"action":"get_session","session_ref":"code:SESSION_ID","offset":0,"limit":20,"max_chars":12000}`
- `{"action":"get_turn","session_ref":"code:SESSION_ID","turn_id":"TURN_ID"}`
- `{"action":"inspect_store","store_id":"code","deep":true}`
- `{"action":"import_openai_export","file_name":"export.zip","dataset_name":"personal"}`

Use the exact `ref` returned by `list_sessions`. Unqualified session IDs fail
with `ambiguous_session` when the same ID occurs in multiple stores. Tool
responses include `ok`; errors include an error code and message.

## Sources and filtering

Discovery includes the active Code home, `~/.code`, `~/.codex`, `CODEX_HOME`, and
`~/.code*/sync` directories. Canonical duplicate roots are removed. Extra roots
can be supplied through `CODE_SESSION_ROOTS`, using the operating system's path
separator. Set `CODE_SESSION_INCLUDE_DEFAULTS=0` to use only the explicit roots
and imported datasets.

Supported sources:

- `state_5.sqlite` for session metadata.
- `thread_history_1.sqlite` for normalized turn/item projections.
- `sessions/**/rollout-*.jsonl` and archived rollout files.
- `history.jsonl` for history-only sessions.
- Existing named OpenAI export datasets.

SQLite projections take precedence over raw rollouts; raw rollouts take
precedence over prompt-only history, avoiding duplicate messages from different
representations. Databases are opened read-only with `query_only` enabled.

`list_sessions` supports `store_id`, `cwd_contains`, `since_ms`, `until_ms`,
`archived`, and `source_kind`. Time bounds refer to creation time in Unix
milliseconds. `search_messages` additionally accepts `session_ref`, `role`
(`user` or `assistant`), and `include_tools`.

Pagination is zero-based. Page sizes default to 50, with a maximum of 200;
search defaults to 20 matches, with a maximum of 100. Retrieval defaults to a
50,000-character content budget, capped at 200,000. Check `truncated` and
`next_offset`; a shortened item includes a truncation marker. The budget covers
message/tool content, not the JSON envelope and session metadata.

## Privacy and imports

System/developer messages, reasoning records, private reasoning channels, and
hidden export messages are excluded. Likely secrets are redacted recursively in
tool arguments/results and in message text using Code's shared sanitizer.
Opaque session, turn, and item identifiers and source paths remain intact so
references and cross-store lookups stay usable.
Redaction is heuristic, not a guarantee that arbitrary personal or confidential
information will be detected. Retrieved history is untrusted data, not a new
instruction to execute.

Imports accept only a filename in the inbox returned by `list_stores`, not an
arbitrary path or URL. Only conversation JSON members are read from ZIP files;
assets and unrelated account files are not extracted. Unsafe member paths,
source symlinks, duplicate IDs, cyclic parent graphs, and existing dataset names
are rejected. Original exports and existing datasets are never overwritten.
The native import action checks the session's write permissions for the data
directory before doing any work.

`CODE_SESSION_DATA_DIR` selects the import directory. Its default remains
`$XDG_DATA_HOME/code-session-mcp`, or `~/.local/share/code-session-mcp`, so
previously imported datasets stay discoverable. Imports write private files on
Unix, preserving branch relationships and `is_active_branch` markers, and omit
non-text media rather than copying attachments.

Import limits are 8 GiB per ZIP, 128 MiB per conversation JSON file, 512 MiB of
conversation JSON per ZIP, 50,000 ZIP entries/conversations, and 100,000 nodes
per conversation. JSONL records are limited to 16 MiB; deep inspection reports
at most 500 rollout files. Limits fail explicitly rather than permitting
unbounded allocation or silently truncating an imported dataset.

The external prior-sessions MCP configuration is not automatically removed.
It can remain available while a newly built native implementation is verified.

## Resource containment

Each native tool request uses a fresh worker with a private input/output pipe.
At most one history worker runs per Code process, including across session tabs.
Other requests wait up to 30 seconds for that resource slot and then return
`resource_busy`; this shared budget does not share conversation data or results.

The worker input is capped at 1 MiB and its complete serialized response at
4 MiB. The parent imposes a 30-second work deadline and a short shutdown grace
period. The worker also arms its own 30-second wall-clock deadline before reading
input, including when invoked directly; inability to start that timer fails
closed. Cancellation or a deadline/output-limit failure terminates and reaps
the worker instead of abandoning an in-process scan. A worker crash returns an
error; it never falls back to unbounded execution inside Code.

Memory enforcement is platform-specific:

- Linux and the Cosmopolitan executable apply a 512 MiB address-space limit
  inside the worker, after exec. Inherited lower hard/soft limits are preserved.
  A CPU-time limit is also set where the runtime enforces it, and Unix core
  dumps are disabled to avoid writing private history into a crash dump.
- The pinned Cosmocc 4.0.2 runtime rejects CPU/core limits on Windows. On that
  host only, the APE worker retains its mandatory address-space limit and uses
  the worker-local wall-clock deadline instead of the unavailable CPU limit.
  It does not ignore address-space enforcement failures or allow unbounded work.
- On Linux, delegated cgroups additionally apply memory and 32-task limits.
  If cgroup memory enforcement is unavailable, the existing process-group RSS
  watchdog provides an additional sampled memory check. Linux workers also
  receive a parent-death signal.
- Native Windows builds use a Job Object with a 512 MiB process-memory limit,
  a CPU-time limit, and one active process. The Cosmopolitan executable uses
  its own resource-limit implementation instead.
- Native macOS builds fail explicitly with `resource_enforcement_unavailable`:
  their address-space limit is not equivalent to the required enforcement.
  This is distinct from Cosmopolitan's userspace address-space limit.

These are worker limits, not a memory reservation or a cap on all of Code and
its tools. Process-group RSS accounting can count shared pages more than once.
The command watchdog covers children that remain in the group; it is not a
replacement for delegated cgroups when descendants detach into other groups,
and it does not supply an equivalent fallback process-count limit.

Imports keep their original export untouched. The supervisor owns a private
staging directory on the dataset filesystem and removes it after normal failure,
timeout, or cancellation, including when the worker cannot unwind. A finished
dataset is published atomically. If a request is interrupted just after that
publication, the complete dataset can exist despite a missing response; inspect
the store list before retrying with the same dataset name. A hard termination of
the parent process itself can still leave staging files for manual inspection.

The direct `code-session-history` library entry point remains blocking and is
not an OS resource boundary by itself. Resource isolation belongs to the native
tool's supervisor and early executable dispatcher.

## Compatibility and verification

Imported-dataset discovery recognizes `code-session-mcp.openai-export.v1`.
Invalid manifests produce discovery warnings rather than an automatic upgrade.
The Rust reader/writer is checked against the local Python MCP reference in both
directions, including alternate branches, tool pairs, and SQLite projections.
Code uses its shared `[REDACTED_SECRET]` marker; the Python reference uses
`[REDACTED]`, so truncation counts reflect each implementation's actual output.

The `code-session-history` crate contains focused parser, privacy, import,
pagination, and filesystem-boundary tests. The core integration test
`session_history_native` exercises all seven operations without an MCP process.
The manual Cosmopolitan build runs that integration test before packaging the
artifact, alongside the conversation identity regression test.
It also runs the worker supervisor and nested-child watchdog regressions. Both
the Linux and Windows artifact checks exercise the hidden worker entry point
against temporary stores, malformed/oversized requests, write-policy refusal,
and a valid JSON import whose decoded allocations exceed the worker budget.
