# Slash Commands

The Code CLI supports a set of slash commands you can type at the start of the
composer input. These commands provide quick actions, toggles, or expand into
full prompts. This document lists all built‑in commands and what they do.

Notes

- Commands are shown in the TUI’s slash popup; the order below matches the UI.
- Commands marked “prompt‑expanding” transform your input into a full prompt and
  typically kick off multi‑agent flows.
- Some commands accept arguments; if required, usage is shown in parentheses.

## Navigation & Session

In the full-screen TUI, the browser-style tab strip sits above the top status
bar. Click a title to select it, its right-hand `×` to close it, or `+` to open
a new session. `Alt+Left` / `Alt+Right` switch tabs even with unsent input
(unless a modal is open). On narrow terminals, arrow buttons navigate tabs and
the selected tab remains visible.

Each session keeps its own draft across tab switches and TUI close/reopen.
Resume that session with `/resume`, or use `/tabs restore` to recover the whole
layout and its drafts. Drafts include multiline text, the cursor, large-paste
payloads, and image references. Submitted or cleared text is not restored.
Drafts and per-folder layouts are saved locally under `CODE_HOME/tui-tabs/`
using atomic writes and private file permissions on Unix. They are plaintext,
not encrypted; image references require the referenced image files to remain
available. A save/load failure is shown in the TUI.

Background shell results, agent updates, and queued widget callbacks stay with
their owning session, including while that tab is hidden. Switching tabs does
not redirect pending output, approvals, or operations to the newly selected tab.
Tabs still share the machine, workspace files, and explicitly shared external
services; they are not filesystem or browser sandboxes.

The AI can use `set_session_title` to choose a short title for the current topic
and update it when the discussion materially changes. This tool can rename only
its own session, and uses the same persistent nickname as `/rename`. Until a
title is chosen, tabs use a neutral label rather than quoting an earlier user
message. The title remains unchanged unless the AI or user explicitly updates it.

- `/browser`: open internal browser.
- `/chrome`: connect to your Chrome browser.
- `/new` or `/tab new`: open a new session tab without cancelling work in other tabs.
- `/tab next` / `/tab prev`: switch to the next / previous visible tab.
- `/tab <N>`: switch to the numbered tab (numbers start at 1).
- `/tab close [N]`: close the current tab, or the numbered tab. This hides the
  tab without deleting the session or its draft, and does not cancel running work.
  Closing the last tab opens a new session.
- `/tabs`: list open tabs and their numbers.
- `/tabs restore`: load the tab layout saved by the previous TUI instance for
  this working folder, reopen its sessions in order, and select its active tab.
  Existing tabs are kept after the restored tabs. Sessions that cannot be found
  or loaded are reported rather than silently replaced with new conversations.
- `/resume`: resume a past session for this folder.
- `/rename <name>`: rename the current session (shown in the resume list).
- `/quit`: exit Code.
- `/logout`: log out of Code.
- `/login`: manage Code sign-ins (select, add, or disconnect accounts).
- `/accounts`: open account switching settings.
- `/secrets`: show how to store API keys and other secrets locally. Use
  `code secrets set OPENAI_API_KEY` to store your key in `CODE_HOME` (encrypted
  at rest). Use `--scope env` for per-repo secrets.
- `/settings [section]`: open the settings panel. Optional section argument
  jumps directly to `model`, `theme`, `interface`, `shell`, `ssh`, `shell-escalation`,
  `shell-profiles`, `exec`, `updates`, `accounts`, `secrets`, `apps`, `agents`,
  `memories`, `prompts`, `personality`, `skills`, `plugins`, `auto`, `review`,
  `planning`, `validation`, `limits`, `chrome`, `mcp`, `repl`, `network`,
  `notifications`.
- `/memories [status|refresh|clear|settings]`: inspect effective Memories
  settings and artifact status, rebuild artifacts immediately, clear generated
  files, or open Settings → Memories.
- `/shell [path]`: configure the shell used for command execution. Without
  arguments, shows the current shell. With arguments, sets the shell path and
  any flags. Use `/shell -` to clear the setting and use auto-detection.
  Examples: `/shell /bin/bash`, `/shell /bin/bash -l`, `/shell /usr/local/bin/fish --interactive`.
- `/ssh`: manage the dynamic saved-PC registry. See [SSH connections](ssh-connections.md).
  Registration is local configuration only; this command does not connect to a host.

## Workspace & Git

- `/init`: create an `AGENTS.md` file with instructions for Code.
- `/diff`: show `git diff` (including untracked files).
- `/undo`: open a snapshot picker so you can restore workspace files to a
  previous Code snapshot and optionally rewind the conversation to that point.
- `/branch [task]`: create a worktree branch and switch to it. If a
  task/description is provided, it is used when naming the branch. Must be run
  from the repository root (not inside another branch worktree). Set
  `CODE_BRANCH_COPY_CACHES=1` (legacy: `CODEX_BRANCH_COPY_CACHES=1`) to mirror
  `node_modules` and Rust build caches into the worktree; otherwise no cache
  directories are copied automatically.
- `/merge`: merge the current worktree branch back into the default branch and
  remove the worktree. Run this from inside the worktree created by `/branch`.
- `/push`: tell Code to commit, push, and monitor workflows with guarded
  instructions. If no workflows appear right away, wait briefly and check again
  before concluding none were triggered. Skips cleanup or GitHub monitoring
  steps automatically when the workspace is already clean or required
  tooling/files are missing.
- `/review [focus]`: without arguments, opens a review picker so you can audit
  the workspace, a specific commit, compare against another branch, or enter
  custom instructions. With a focus argument, skips the picker and uses your
  text directly. Configure Auto Resolve and the max re-reviews (defaults to 5)
  from `/settings review` when you want Code to rerun fixes and follow-up
  checks automatically.
- `/cloud`: browse Code Cloud tasks, view details, apply patches, and create
  new tasks from the TUI.
- `/cmd <name>`: run a project command defined for the current workspace.

## UX & Display

- `/theme`: customize the app theme.
- `/verbosity (high|medium|low)`: change text verbosity.
- `/model`: choose your default model. The picker also groups models from
  configured OpenAI-compatible endpoints and provides
  **+ Add OpenAI-compatible /v1 endpoint**. Direct selection changes the current
  session's provider and model without creating or changing a profile; see
  [TUI Settings Overlay](settings.md#openai-compatible-endpoints).
- `/fast`: open the model selector and toggle Fast mode.
- `/reasoning (minimal|low|medium|high)`: change reasoning effort.
- `/prompts`: manage custom prompts.
- `/skills`: manage skills.
- `/apps`: browse and use apps (connectors). Installed apps can be inserted as
  `$<app>` mentions.
- `/status`: show current session configuration and token usage.
- `/statusline [primary|secondary|top|bottom]`: choose and reorder built-in
  status fields. Uses the configured primary lane by default and supports
  explicit top/bottom deep links.
- `/limits`: adjust session limits and visualize hourly and weekly rate-limit
  usage.
- `/update`: check the installed version, detect available upgrades, and open a
  guided upgrade terminal that runs the installer interactively when possible.
- `/notifications [status|on|off]`: manage notification settings. Without
  arguments, shows the notifications panel. With arguments: `status` shows
  current config, `on` enables all, `off` disables all.
- `/mcp [status|on|off <name>|add]`: manage MCP servers. Without arguments,
  shows all servers with toggle controls. With arguments: `status` lists
  servers, `on <name>` enables, `off <name>` disables, and `add` starts the new
  server workflow.
- `/validation [status|on|off|<tool> (on|off)]`: inspect or toggle validation
  harness settings.

## Search & Mentions

- `/mention`: mention a file (opens the file search for quick insertion).

## Performance & Agents

- `/perf (on|off|show|reset)`: performance tracing controls.
- `/agents`: configure agents and subagent commands (including autonomous
  follow-ups and observer status; available in dev, dev-fast, and perf builds).
- `/auto [goal]`: start the maintainer-style auto coordinator. If no goal is
  provided it defaults to "review the git log for recent changes and come up
  with sensible follow up work".

## Prompt‑Expanding (Multi‑Agent)

These commands expand into full prompts (generated by `code-core`) and
typically start multiple agents. They require a task/problem description.

- `/plan <task>`: create a comprehensive plan (multiple agents). Prompt‑expanding.
- `/solve <problem>`: solve a challenging problem (multiple agents). Prompt‑expanding.
- `/code <task>`: perform a coding task (multiple agents). Prompt‑expanding.

## Development‑Only

- `/demo`: populate the chat history with assorted sample cells (available in
  dev and perf builds for UI testing).
- `/demo auto drive card`: render the Auto Drive card once for each ANSI-16
  background color so you can compare theme contrast.
- `/test-approval`: test approval request (available in debug builds only).

Implementation Notes

- The authoritative list of commands is defined in
  `code-rs/tui/src/slash_command.rs` (the `SlashCommand` enum). When adding a
  new command, please update this document to keep the UI and docs in sync.
- Prompt formatting for `/plan`, `/solve`, and `/code` lives in
  `code-rs/core/src/slash_commands.rs`.
  When no `[[agents]]` are configured, the orchestrator advertises the
  following model slugs to the LLM for multi-agent runs: `code-gpt-5.4`,
  `code-gpt-5.3-codex`, `claude-opus-4.6`, `gemini-3-pro`,
  `code-gpt-5.1-codex-mini`, `claude-sonnet-4.5`, `gemini-3-flash`,
  `claude-haiku-4.5`, and `qwen-3-coder` (with
  `cloud-gpt-5.1-codex-max` gated by `CODE_ENABLE_CLOUD_AGENT_MODEL`). (`gemini`
  resolves to `gemini-3-flash`.) You can replace or pin this set via
  `[[agents]]` or per-command `[[subagents.commands]].agents`.
