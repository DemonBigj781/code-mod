# Native session-history

Code exposes `session_history` as a built-in Rust agent tool. It implements the
prior-sessions operations without starting Python, an MCP server, or a proxy.
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
