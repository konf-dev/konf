# konf-tool-runner

Runner tool family for konf.

A **runner** is a thing that can start a workflow as an asynchronous run,
track its lifecycle, and let callers query status, block for completion,
or cancel it. Four tools are registered by this crate:

| Tool             | Shape                                                                 |
|------------------|-----------------------------------------------------------------------|
| `runner:spawn`   | `{workflow, input?}` → `{run_id, workflow, backend}`                  |
| `runner:status`  | `{run_id}` → `RunRecord` (non-blocking)                               |
| `runner:wait`    | `{run_id, timeout_secs?}` → `RunRecord` (blocks until terminal)       |
| `runner:cancel`  | `{run_id}` → `{run_id, cancelled}`                                    |

A `RunRecord` is a flat JSON object: `{id, workflow, backend, state, created_at, started_at?, finished_at?}` with an optional `result` (success) or `error` (failure) flattened alongside `state`.

## v1 scope

- **`InlineRunner`** — runs each workflow as a tokio task inside the same
  process, resolved from the engine's registry as `workflow:<name>`. Cheap,
  no isolation, no resource limits. This is the floor that makes
  composition work.
- **Registered unconditionally** by `konf-init` at boot time (step 9d in
  `konf-init/src/lib.rs`). Every product built against `konf-init`
  automatically has the four runner tools available.

## Deferred (documented in `konf-experiments/findings/017-runner-is-a-tool-family.md`)

- **`SystemdRunner`** — user-scope transient units with cgroup limits and
  journald logs. Adds process-level isolation on Linux.
- **`DockerRunner`** — cross-platform container isolation via `bollard`.
- **`runner:logs`** — streaming stdout/stderr; today the terminal
  `RunRecord.result` carries any captured output.

Each future backend is one new crate module + one impl of the `Runner`
trait. They plug into the existing shared `RunRegistry` so
`runner:status`/`wait`/`cancel` keep working across backends.

## Why a tool family, not a kernel primitive

Finding 014 established that the only new *kernel* primitive needed for
autonomous agents is the `schedule` tool — which is itself a tool, not a
trait in `konf-runtime`. The runner follows the same principle: it
extends what konf can do by shipping a tool crate, not by modifying the
engine.

The `Runner` trait that lives in `src/runner.rs` is an internal
abstraction for this crate's backends. Callers never see it; they see
the four tools.

## Usage

```rust
use std::sync::Arc;
use konf_tool_runner::{register, InlineRunner, RunRegistry, Runner};

let runtime: Arc<konf_runtime::Runtime> = /* ... */;
let registry = RunRegistry::new();
let inline: Arc<dyn Runner> = Arc::new(InlineRunner::new(
    runtime.clone(),
    registry,
));
register(runtime.engine(), inline)?;
```

After `register()`, workflows can use `runner:spawn` / `runner:wait` like
any other tool. See `konf/products/ci/config/workflows/gate.yaml` for a
fan-out example.

## Tests

```bash
cargo test -p konf-tool-runner
```

10 integration tests across two files (`tests/inline_runner.rs`,
`tests/tool_surface.rs`) exercise every tool and every runner-trait
method end to end using a real `Engine` + `Runtime` and tiny
test-workflow tools. No mocking.

## License

BUSL-1.1. See [LICENSE](../../LICENSE).
