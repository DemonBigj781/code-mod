# Native Memories Retrieval

Code exposes `memories` as a built-in, read-only agent tool for curated guidance
from prior runs. The tool resolves `CODE_HOME/memories/current` on every call and
uses stable logical artifact names, so prompts and agents do not need to retain
or grep a generation-specific snapshot directory.

## Operations

- `status`: report the current generation, available artifacts, and rollout
  summary count.
- `search`: perform a literal, case-insensitive line search. `source` can be
  `all`, `summary`, `memories`, or `rollouts`.
- `read`: read a bounded line range from a logical artifact path.
- `list_rollouts`: page through available rollout-summary metadata.

The supported logical read paths are:

- `memory_summary.md`
- `raw_memories.md`
- `rollout_summaries/<slug>.md`

`read` defaults to 200 lines and 50,000 returned characters. `search` defaults
to 20 matches and 50,000 returned characters. Requests are bounded to 100
results, 2,000 lines, and 200,000 returned characters; search also caps the
amount of curated material scanned in one call.

## Memories versus session history

Use `memories` for curated, reusable guidance. Use
[`session_history`](session-history.md) when exact prior messages, commands,
errors, tool records, or sessions from another working directory are needed.
Neither tool requires generic filesystem grep over `CODE_HOME`.

Published memory generations are generated artifacts. Callers should not edit
snapshot directories directly or assume an old generation path will remain
available after a refresh.
