# Durability in konf

> Single source of truth for what "durable" means in konf, and what it
> explicitly does not mean. If any other doc contradicts this one,
> this doc wins.

## The rule

Konf's durability model has two parts:

1. **Durable intent.** The fact that "workflow X should run with input
   Y" is persisted to disk — in the scheduler's timer table, in the
   runner intent table, or as a `schedules.yaml` entry.
2. **Idempotent retry.** On restart (or on crash mid-flight), the
   unterminated intents are re-run **from the top** with the original
   input. Never mid-workflow resume. Never checkpoint-and-replay.

Workflow authors are responsible for making workflows idempotent when
they have side effects. This is the entire contract.

## What this is not

Konf explicitly rejects Temporal/Cadence-style durable execution.
That model saves `(step_name, step_output)` pairs and skips completed
steps on replay. It does not work for AI agents because LLM calls are
non-deterministic — replaying from a mid-workflow checkpoint produces
a different answer than the original run, and any downstream logic
that depended on the original answer becomes incoherent.

We do not:

- save intermediate variables
- save tool-call history
- skip completed nodes on replay
- track execution state beyond the in-memory [`ProcessTable`]
- resume a workflow at the node it crashed on

The only thing we persist across restarts is the **original input** and
enough metadata to re-launch the workflow from node zero.

## The three persistent stores

All three live in a single redb file managed by
[`konf_runtime::KonfStorage`](../../crates/konf-runtime/src/storage.rs).
Configure the file path via `konf.toml`:

```toml
[database]
url = "redb:///var/lib/konf/konf.redb"
retention_days = 7
```

### Event journal

Append-only audit log of every workflow lifecycle event
(`workflow_started`, `node_started`, `node_completed`, `workflow_failed`, …).
Queried by `/v1/admin/audit` and by `Runtime::journal().query_by_run()`.

On boot, `Runtime::new` calls `journal.reconcile_zombies()` which scans
the log for runs that started but never reached a terminal event and
inserts a synthetic `workflow_failed` with reason "System restart —
workflow was interrupted". This makes crashed runs visible in the audit
trail without pretending they finished.

See [`konf_runtime::journal`](../../crates/konf-runtime/src/journal.rs).

### Scheduler timers

Key: `(fire_at_unix_ms, job_id)` → postcard-encoded `StoredTimer`.

Three modes:

- `TimerMode::Once` — fire at a specific time, then delete.
- `TimerMode::Fixed { delay_ms }` — fire every `delay_ms`, rescheduled
  after each fire. Bounded to `1s..=7 days`.
- `TimerMode::Cron { expr }` — fire at each match of a 7-field cron
  expression (second min hour day month weekday year). Re-parsed at each
  fire to compute the next time.

Polling is `timers.range(..now_ms)` under a read transaction — O(due),
not O(total). The poll loop ticks every second by default.

When a timer fires, the scheduler resolves `workflow:<id>` in the
**live engine registry** (not a snapshot) and invokes it. This means
editing the workflow YAML and calling `config:reload` takes effect on
the next fire — you don't have to cancel and re-schedule.

If the referenced workflow has been removed, the scheduler logs a
visible error event on the bus and **leaves the timer in place** for
future retries. Re-registering the workflow resumes normal firing.

See [`konf_runtime::scheduler`](../../crates/konf-runtime/src/scheduler.rs).

### Runner intents

Key: `run_id` (UUID v4 string) → postcard-encoded `StoredIntent`.

Every call to `runner:spawn` (when a `KonfStorage` is configured)
writes an intent with `terminal: None` **before** the tokio task
starts. On successful completion the task writes `terminal: Some(...)`
back.

On boot, `konf-init` calls `intents.list_unterminated()` and replays
every entry by calling `InlineRunner::replay(intent)`. The replay:

1. Increments `replay_count` and re-persists the intent.
2. If `replay_count > 10`, marks the intent `Failed { error: "replay
   loop exceeded limit" }` and stops — prevents crash loops from
   eating boot time forever.
3. Otherwise spawns the workflow via `InlineRunner::spawn_with_id`
   using the **same run id** as the original call. TUI bookmarks and
   journal entries that reference the old run id still resolve after
   the replay.

Terminal entries are garbage-collected after `retention_days` days by
a background task on `KonfStorage`.

See [`konf_runtime::runner_intents`](../../crates/konf-runtime/src/runner_intents.rs).

## Worked example: an email watcher

You want to poll your inbox every minute, summarize new messages, and
write a digest to `~/notes/inbox.md`. The workflow looks like:

```yaml
# workflows/check-inbox.yaml
workflow: check_inbox
register_as_tool: true
capabilities: [memory:*, mcp:gmail:*, mcp:fs:*, ai:complete]
nodes:
  load_cursor:
    do: memory:search
    with: { namespace: "inbox_cursor", key: "last_checked_at" }
  fetch:
    do: mcp:gmail:list_since
    with: { since: "{{ load_cursor.value }}" }
  summarize:
    do: workflow:summarize_emails
    with: { emails: "{{ fetch.results }}" }
    return: true
  save_cursor:
    do: memory:store
    with: { namespace: "inbox_cursor", key: "last_checked_at", value: "{{ now }}" }
```

And the schedule:

```yaml
# schedules.yaml
- name: inbox_check
  workflow: check_inbox
  cron: "0 * * * * * *"   # every minute
  namespace: "konf:assistant:bert"
  capabilities: [memory:*, mcp:gmail:*, mcp:fs:*, ai:complete]
  input: { user: bert }
```

Normal operation:

1. At 12:00, scheduler polls, finds the cron due, fires `check_inbox`.
2. `check_inbox` reads `last_checked_at` from memory (11:59), calls
   `gmail:list_since(11:59)`, gets 3 new emails, summarizes them,
   stores the summary, updates `last_checked_at` to 12:00. Duration: 2
   seconds.
3. Workflow exits. Scheduler reschedules for 12:01.

Crash mid-run:

1. At 14:00, `check_inbox` fires. Workflow fetches emails 1/3, 2/3, and
   crashes on email 3 (your laptop force-reboots).
2. Konf-backend is killed. `RunnerIntent { terminal: None }` is still
   in redb. Scheduler timer's next fire is 14:01, also still in redb.
3. Laptop comes back at 14:02.
4. `konf-init::boot` calls `journal.reconcile_zombies()` → inserts
   `workflow_failed { reconciled: true }` for run_14:00. The in-memory
   `ProcessTable` is empty.
5. `konf-init::boot` calls `intents.list_unterminated()` — finds the
   `check_inbox` intent from 14:00. Calls `InlineRunner::replay(...)`
   with the same run id. Workflow runs from the top.
6. The workflow reads `last_checked_at` from memory, still at 13:59
   (because run_14:00 crashed before `save_cursor`). It re-fetches the
   same emails it was already processing. The summarize step is
   idempotent via memory lookups — it sees it already summarized
   emails 1 and 2, skips them, summarizes 3, writes the cursor,
   exits.
7. Meanwhile, the scheduler polls at 14:03, sees the 14:01 fire is
   overdue, and fires again — catch-up semantics. The same replay
   logic applies.

This is "at-least-once with idempotent retry", not mid-workflow
resume. The workflow code is responsible for the idempotency — konf
provides the durability scaffolding.

## Idempotency patterns

Three common patterns for writing workflows that survive replay:

**Memory-backed cursors.** Read a `last_processed_id` from memory at
the start, write it at the end of a successful run. If you crash
between, replay starts from the last saved cursor, not the original
start. Works for email, RSS, logs, any monotonic stream.

**Dedup keys.** Before calling an irrevocable tool (send email, create
invoice, post to Slack), check memory for an idempotency key derived
from the input. If present, skip. Store the key after the call
succeeds. Cheap on cold cache (one memory read per call).

**Small composed workflows.** Break "fetch → summarize → publish" into
three separate workflows chained via `workflow:*` tool calls. Each
sub-workflow is individually idempotent and has a tight timeout. On
crash replay, only the currently running sub-workflow re-executes,
not the whole chain. The orchestrator workflow reads memory to see
which sub-workflows already produced their output.

See also:
[`konf_runtime::event_bus`](../../crates/konf-runtime/src/event_bus.rs)
for the live event stream your TUI subscribes to at
`/v1/monitor/stream`.
