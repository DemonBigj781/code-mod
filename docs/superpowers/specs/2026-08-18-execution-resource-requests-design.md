# Execution Resource Requests Design

<!-- markdownlint-disable MD013 -->

## Goal

Allow the model to request a temporary increase to Code-managed execution
limits after a tool command cannot run within the current memory or process
allowance. The user remains the authority for every increase.

## Scope

The initial resource profile MUST cover the limits Code currently enforces:

- `memory_max_mb`
- `pids_max`

The feature MUST NOT claim to increase host RAM, container limits, systemd
limits, or any outer cgroup limit. CPU parallelism is outside this edition and
all project compilation remains restricted to one worker.

## Tool Interface

Add a built-in `request_resources` tool with this logical payload:

```text
memory_max_mb: optional integer
pids_max: optional integer
reason: optional string
```

At least one resource MUST be requested. Values MUST be greater than the
effective current limit unless the current limit is disabled. The tool MUST
show the current and requested values before waiting for approval.

## Approval Behavior

The approval UI MUST offer:

- Allow for the next tool-spawned command.
- Allow for the rest of this session.
- Deny.

A next-command grant MUST be consumed atomically by the next shell or process
execution attempt. A session grant MUST remain in memory only and MUST NOT
rewrite `config.toml`. Denied requests MUST leave the effective limits
unchanged.

The requested values MUST be clamped to any known outer hard limit. When an
outer limit cannot be determined, the UI MUST state that approval changes only
Code's own limit and does not guarantee the operating system will allow it.

## Failure Guidance

When Code terminates a command for exceeding its managed memory or process
limit, the tool result MUST include:

- The limit that was exceeded.
- The effective value.
- A machine-readable resource-limit failure classification.
- Guidance that the model MAY call `request_resources` before retrying.

Code MUST NOT automatically retry a command after approval. The model MUST
issue a new command so the retried action is visible in conversation history.

## State And Enforcement

Resource grants MUST be stored on the active session and included when Code
constructs the effective `ExecCgroupLimits` for a new command. Both unified
exec and persistent shell sessions MUST use the same resolution path.

Precedence MUST be:

1. Applicable approved one-shot grant.
2. Applicable approved session grant.
3. Configured `[exec_limits]` value.
4. Existing automatic default.

The implementation MUST test both increasing memory and increasing PID limits.
It MUST also test that a one-shot grant is not accidentally reused.

## Verification Scenarios

- GIVEN a command exceeds `memory_max_mb`, WHEN the user grants a larger
  one-shot limit and the model retries, THEN only the retried command receives
  the larger limit.
- GIVEN a session-scoped PID grant, WHEN two commands run, THEN both receive the
  approved value.
- GIVEN a denied request, WHEN the model retries, THEN the original limit still
  applies.
- GIVEN an outer cgroup lower than the requested value, WHEN the request is
  approved, THEN Code reports the effective clamp and does not claim the full
  value was granted.

## Deferred Work

Prompt compression and AI-assisted context reduction are explicitly deferred
until their behavior is specified separately.
