# konf audit — resolution plan

Companion to `issues-prd.md`. For every issue the auditor found, this
document states the exact fix, the files it touches, and whether a
design decision is needed from the user before work can start.

All work lands on branch `audit/prd-discovery` (or a branch off it),
never on `main`. `main` keeps the current shipped Phase 1–3 code until
the rebuild is merged.

**Rule**: no backward compatibility. Code and docs are rewritten to be
the best version of themselves, not the diff-minimising version.

---

## Ordering

1. **Pre-work** (no decisions): verify `@0@` syntax in SurrealDB 3.x, confirm konflux-core has no iteration primitive, confirm `konf-init-kell` is self-referential. All three **done** above.
2. **Design decisions** — user answers 4 questions before any code moves.
3. **Fixes by area**, grouped so each commit is a self-contained slice:
   - A. Runner registry (C1, L3, L4 indirectly)
   - B. SurrealDB backend correctness (H1, H2, H3, M1, M3)
   - C. Docs + vocabulary (C3, H4, H5, M2, M5, M7, L1, L2, N1, N3)
   - D. Products & workflows (C2)
   - E. Dead code cleanup (M4, M6, N4)
4. **Verification**: full workspace `fmt` / `test` / `clippy`, re-run the SurrealDB experiment, re-run the runner tests, manual inspection pass.

---

## Design decisions needed (4)

These are the only items where I'm not 95% sure what you want. Everything
else has a proposed fix documented inline below and will be executed
unless you override in review.

### Q1 — C2. `gate` workflow scope

The current `gate.yaml` claims to iterate over `input.crates` but does
not. konflux-core has no iteration primitive, so real per-crate fan-out
requires either extending the engine or rewriting the workflow.

- **(a) Flat fixed gate** — drop `input.crates` entirely. `gate.yaml`
  spawns three runs in parallel: `cargo_fmt_check`, `cargo_clippy`,
  `cargo_test_workspace` (new workflow that runs `cargo test --workspace`
  once, not per-crate). Smallest scope, honest, no engine change.
- **(b) Extend konflux-core with a `for_each` node** — real iteration
  primitive added to the YAML schema and the compiler. `gate.yaml` then
  iterates over `input.crates` for the test step. Larger change,
  touches parser/compiler/validator, but gives workflows a capability
  they currently lack. Cleanest product story.
- **(c) Multi-gate per-crate** — caller passes `crate:` on every invocation;
  `gate.yaml` is a single-crate check. Drop the "gate the whole workspace"
  story. Caller orchestrates.

My recommendation: **(a)** for now. Adding iteration to konflux-core
(option b) is a real feature and deserves its own plan cycle with a
dedicated finding. A flat 3-run gate is honest, useful, and ships in 10
minutes. Option (c) pushes orchestration burden onto the caller and
erases the demo.

### Q2 — H1. `is_retracted` (soft-delete) semantics

`is_retracted` exists in the schema and is filtered by `text_search` but
not by `vector_search`. There is no API to set it — no `MemoryBackend`
method, no tool, no caller. It's latent YAGNI.

- **(a) Drop it entirely.** Remove the column from the schema, remove
  the filter from `text_search`. Hard deletes only. Smallest surface.
- **(b) Keep it, add a `retract_node` method** to `MemoryBackend`, fix
  `vector_search` to filter it consistently, add a `runner:retract` tool
  or similar. Real soft-delete story.
- **(c) Keep schema, drop the filter.** Admit it's unused, stop
  pretending `text_search` honours it. Confusing middle ground.

My recommendation: **(a)**. No caller exists. It's speculative. The
text-search filter asymmetry with vector-search is a symptom, not the
problem — the problem is that the whole feature is undocumented and
unused. The `MemoryBackend` trait can grow `retract_node` later in its
own change if it's ever wanted.

### Q3 — M7. `konf-init-kell` crate rename

The crate name and binary contain the banned word "kell" (per
`MENTAL_MODEL.md` kill list), its `Cargo.toml` description uses the
banned word, and the code inside the crate references `kell_dir`
throughout. Nothing else in the monorepo depends on the crate (grep:
only self-references and one audit-PRD mention).

- **(a) Rename to `konf-scaffold`**, rename `main.rs` variables, fix
  description. Full cleanup.
- **(b) Rename to `konf-new`** (matching `cargo new`). More conventional.
- **(c) Keep the crate name `konf-init-kell` but fix description +
  internal `kell_dir` → `product_dir`.** Half-measure — binary name
  stays vestigial, but all user-facing strings lose the banned word.
  Current MENTAL_MODEL explicitly accepts this.
- **(d) Delete the crate.** It's a CLI scaffolder; not load-bearing.
  Products can be created by copying `products/_template/` manually.

My recommendation: **(a)**. The user has explicitly said "no backward
compatibility", and the crate has zero external callers. A clean
rename is cheaper than carrying a banned-word exception forever.
Option (d) is tempting but the scaffolder does have value for new
products.

### Q4 — L3. Runner registry eviction policy

`RunRegistry` currently grows without bound. Terminal runs are never
removed. A long-running `konf-mcp` serving `products/ci/` will accumulate
every run forever.

- **(a) Size-bound FIFO** — keep last 1000 terminal runs; evict oldest
  when over. No config, no decisions at runtime.
- **(b) TTL-based** — evict terminal runs older than 1 hour. Configurable
  via `tools.yaml` (`runner.retention_secs: 3600`). Slightly more code.
- **(c) Explicit `runner:forget`** — caller is responsible for cleanup.
  Add a fifth tool. Most honest, least ergonomic.
- **(d) Automatic on wait** — once `runner:wait` returns a terminal
  record, the entry is evicted. Clever but surprising: `runner:status`
  after `runner:wait` returns "not found".
- **(e) Combination of (a) + (c)** — automatic FIFO bound AND explicit forget.

My recommendation: **(a)** for v1. 1000 runs is well above any
reasonable fan-out. Simplest thing that works. Add `runner:forget` in a
follow-up only if a caller actually needs deterministic cleanup.

---

## Pre-work results (already verified)

- **konflux-core iteration primitive**: NOT PRESENT.
  `grep -rn 'for_each\|iterate' crates/konflux-core/src/` returns only
  an unrelated `wait_for` field. `konflux-core/src/parser/schema.rs`
  has no `for_each` / `each` / `map` node type. Confirms Q1 needs a
  real decision.
- **konf-init-kell external usage**: NONE.
  `grep -rn 'konf-init-kell\|konf_init_kell' crates/ products/ scripts/`
  returns only self-references and the audit PRD. Safe to rename.
- **is_retracted callers**: NONE.
  Schema defines it, `text_search` filters it, nothing sets it. Pure
  latent YAGNI.
- **SurrealDB 3.x `@0@` operator**: will verify via Context7
  (`docs.surrealdb.com` library) before touching `search.rs`. If it's
  legacy syntax, `text_search` is either falling back to a table scan
  or silently failing against the `FULLTEXT` index. Fix lands as part of
  area B.

---

## Fix plan, by issue

### CRITICAL

#### C1. `wait_terminal` lost-wakeup race
**Files**: `crates/konf-tool-runner/src/registry.rs:173-181`
**Fix**: Use the canonical `tokio::sync::Notify` pattern — acquire the
`Notified` future **before** checking state, enable it, then check,
then await. Any `notify_waiters()` that fires after `enable()` is
guaranteed to wake the pinned future.

```rust
pub async fn wait_terminal(&self, id: &RunId) -> Option<RunRecord> {
    let slot = self.slot(id)?;
    loop {
        // Arm the notification BEFORE checking state. This closes the
        // lost-wakeup window: any notify_waiters() call after enable()
        // wakes this future reliably, regardless of scheduling order.
        let notified = slot.done.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        let current = slot.record.read().await.clone();
        if current.state.is_terminal() {
            return Some(current);
        }
        notified.await;
    }
}
```

Also add a regression test that deliberately races completion and waiter
(`tokio::spawn` the waiter, immediately call `mark_terminal`, assert the
waiter returns within a tight timeout). Loop 1000 iterations to catch
any residual race with reasonable confidence.

**Decision required**: no.

#### C2. `gate.yaml` false claim
**Files**: `products/ci/config/workflows/gate.yaml`, possibly
`products/ci/README.md`, possibly `products/ci/config/workflows/` (new
workflows).
**Fix**: depends on **Q1**. Stub written for each option:

- **Q1(a)**: rewrite `gate.yaml` input schema to take no arguments.
  Spawn three runs via `runner:spawn`, wait for all three, return a
  structured `{fmt, clippy, test}` object with each child's terminal
  state. Add a new `cargo_test_workspace.yaml` that runs
  `cargo test --workspace`. README description updated to match.
- **Q1(b)**: extend konflux-core with a `for_each` node (new commit
  on its own), then rewrite `gate.yaml` to iterate. At least a day of
  real work.
- **Q1(c)**: rewrite `gate.yaml` to require `input.crate` (singular),
  return one crate's full ship-gate result. Drop the "gate" framing;
  rename to `ship_one`.

**Decision required**: **yes, Q1**.

#### C3. `MENTAL_MODEL.md` Postgres claim
**Files**: `docs/MENTAL_MODEL.md:57-59` and the follow-on
`docs/MENTAL_MODEL.md` lines that contradict SurrealDB-as-default.
**Fix**: Replace with the honest current state:

> konf has no hard external dependencies. The default memory backend
> (`konf-tool-memory-surreal`, embedded RocksDB) runs entirely in-process.
> Postgres is only required if you opt into `konf-tool-memory-smrti`
> (via `--features memory-smrti`) or the optional Postgres-backed
> scheduler in `konf-backend`. See `crates/konf-tool-memory-surreal/` for
> the default; see smrti for the Postgres path.

Also audit the rest of MENTAL_MODEL.md for other places that still
assume Postgres. Grep for "postgres", "pgvector", "sqlx". Rewrite each
reference honestly.

**Decision required**: no.

---

### HIGH

#### H1. `vector_search` missing `is_retracted` filter
**Files**: `crates/konf-tool-memory-surreal/src/search.rs`, possibly
`crates/konf-tool-memory-surreal/src/schema.rs`.
**Fix**: depends on **Q2**.

- **Q2(a)** (recommended): remove `is_retracted` from `schema.rs`
  (both `node` and `edge` definitions), remove the filter from
  `text_search`. Hard deletes only; the trait currently has no delete
  method for nodes anyway, so the contract doesn't change. Update the
  README feature matrix to drop the "soft-delete" row.
- **Q2(b)**: add `AND node.is_retracted = false` to `vector_search`'s
  WHERE clause. Also add a `MemoryBackend::retract_node(id)` trait
  method, implement it for the SurrealDB backend, add a test that
  retracts a node and asserts it disappears from all three search modes.
  Leave smrti's bridge with an `Unsupported` stub for now. Also add a
  `retracted_edges` query helper and fix the `active_edges` semantics.

**Decision required**: **yes, Q2**.

#### H2. `add_nodes` non-atomic with embeddings
**Files**: `crates/konf-tool-memory-surreal/src/backend.rs:82-190`
**Fix**: Wrap the **entire batch** of node + embedding CREATEs in a
single SurrealDB transaction so either all nodes persist or none do:

```surql
BEGIN TRANSACTION;
    CREATE node SET namespace = $ns1, ...; -- node 1
    CREATE node_embedding SET ...;          -- node 1 embedding (if any)
    CREATE node SET namespace = $ns2, ...; -- node 2
    ...
    CREATE event SET namespace = $ns, event_type = "nodes_added", payload = $meta;
COMMIT TRANSACTION;
```

Rework `add_nodes` to build the full statement list once, bind all
parameters, and issue a single `db.query(...)` call inside
`BEGIN`/`COMMIT`. The per-node loop in Rust goes away; the loop becomes
statement-building. Event row is appended in the same transaction so
the audit log stays consistent with the data.

Add an integration test that forces a mid-batch failure (wrong embedding
dimension on node 3 of 5) and asserts nodes 1, 2, 4, 5 did NOT persist.

**Decision required**: **yes, Q3** — but only on whether whole-batch
atomicity is desired. I strongly recommend it. If the answer is "per-node
only", the fix is smaller and only wraps `node + embedding` in a
transaction, leaving the outer loop to commit one node at a time.

#### H3. `state_set` DELETE-then-CREATE race
**Files**: `crates/konf-tool-memory-surreal/src/session.rs:59-117`
**Fix**: Wrap DELETE + CREATE in a SurrealDB transaction so the two
statements are atomic against concurrent writers:

```surql
BEGIN TRANSACTION;
    DELETE session_state WHERE namespace = $ns AND session_id = $sid AND skey = $k;
    CREATE session_state SET namespace = $ns, session_id = $sid, skey = $k, sval = $v, expires_at = $exp;
COMMIT TRANSACTION;
```

Also compute `expires_at` in Rust (see M3) so the SurrealQL has no
conditional branches and is a single static statement block per call.

Add a race test: two concurrent `state_set` calls for the same key from
different `tokio::spawn` tasks; assert the final value is one of the
two (not an error, not a duplicate).

**Decision required**: no (assuming Q3 doesn't force per-statement).

#### H4 + H5. `docs/architecture/init.md` stale
**Files**: `docs/architecture/init.md` (whole file).
**Fix**: Rewrite the file from scratch against the current `konf-init/src/lib.rs`
`boot()` function. Every claim cites `file:line`. Structure:

1. What `boot()` does in one paragraph.
2. The `KonfInstance` struct — copy-paste from current code with
   `// ← current` annotations.
3. The boot sequence as numbered steps matching the current code's own
   comments exactly (including 9b/9c/9d).
4. Each step's purpose in one sentence.
5. What happens if a step fails (error propagation model).
6. A "this doc's source of truth" line pointing at
   `crates/konf-init/src/lib.rs:`.

Delete everything else in the file. Small doc, citations everywhere.

**Decision required**: no.

---

### MEDIUM

#### M1. `resolve_namespace(None)` ignores `cfg.namespace`
**Files**: `crates/konf-tool-memory-surreal/src/backend.rs`,
`crates/konf-tool-memory-surreal/src/config.rs`.
**Fix**: The confusion is that `cfg.namespace` currently means **two
things at once**: (a) the SurrealDB engine's `USE NS` scope, and (b) the
default multi-tenancy namespace when callers don't pass one. That's
confusing but actually fine as long as the code treats them as the same.

Proposed change:
- Rename `DEFAULT_NAMESPACE` constant to `FALLBACK_NAMESPACE` and keep
  it as the last-resort default when `cfg.namespace` is somehow empty
  (shouldn't happen because config has a default, but belt-and-braces).
- `resolve_namespace(None)` returns `self.cfg.namespace.clone()` when
  set and non-empty, else `FALLBACK_NAMESPACE`.
- Document the dual role explicitly in the `SurrealConfig` doc comment:
  "`namespace` is used both as the SurrealDB `USE NS` scope AND as the
  default value of the `namespace` parameter for `MemoryBackend`
  operations. Callers that need a different tenant namespace MUST pass
  it explicitly."

Drop `DEFAULT_NAMESPACE = "default"`; the default is now the value
from the user's config.

**Decision required**: no (this is the plain correct fix; I can't think
of a reasonable alternative).

#### M2. "five tools" comment
**Files**: `crates/konf-tool-runner/src/lib.rs:16-38` and the
`konf-tool-runner/README.md`.
**Fix**: Count fixed. Update "five tools" → "four tools". Also update
the "callers never see the trait" claim (see L1): the trait is re-exported
and IS part of the public API for anyone wanting to write a new backend.
The four tools are the primary way to USE the runner, but the trait is
the way to EXTEND it.

**Decision required**: no.

#### M3. Fragile TTL string concatenation
**Files**: `crates/konf-tool-memory-surreal/src/session.rs:76-113`
**Fix**: Compute `expires_at` in Rust as a `chrono::DateTime<Utc>` and
bind it directly. No SurrealQL type coercion, no duration parsing, no
string concatenation:

```rust
let expires_at: Option<chrono::DateTime<Utc>> = ttl.and_then(|secs| {
    if secs > 0 {
        Some(chrono::Utc::now() + chrono::Duration::seconds(secs))
    } else {
        None
    }
});
// ...
q.bind(("exp", expires_at))
```

The SurrealQL becomes a single static block with `expires_at = $exp`
(NONE or a datetime — SurrealDB accepts both). Test with TTL=0, TTL=1,
TTL negative (rejected upstream), TTL null.

**Decision required**: no.

#### M4. `reapply_schema` dead code
**Files**: `crates/konf-tool-memory-surreal/src/connect.rs:101-117`
**Fix**: Delete the function and its `#[allow(dead_code)]` attribute.
If a recovery path ever genuinely wants to re-apply the schema, the
caller can just call `build_schema(cfg)` and issue the resulting string
through `db.query()` themselves — it's three lines.

**Decision required**: no.

#### M5. "12-step" docstring lists 11 steps
**Files**: `crates/konf-init/src/lib.rs:50-61`
**Fix**: Rewrite the docstring to:
- Match reality: count the actual numbered steps (including 9b/9c/9d).
- Cite each step's code line: `// 1. Load config (line 82)`.
- Drop the misleading "12-step" framing if the count differs.

I'll either make it say "14-step" or drop the count entirely and just
list them.

**Decision required**: no.

#### M6. `resp_count` + `MergedCount` trait
**Files**: `crates/konf-tool-memory-surreal/src/search.rs`
**Fix**: Delete both. The `MergedCount` trait extension and `resp_count`
were workarounds for a construction-order problem that no longer needs
to exist. Rewrite `text_search` to compute `count` inline after the
rows are materialized:

```rust
Ok(json!({
    "results": rows.clone(),
    "_meta": { "mode": "text", "namespace": ns, "count": rows.len() }
}))
```

`vector_search` already does this correctly. `hybrid_search` already
does this correctly. Bring `text_search` in line.

**Decision required**: no.

#### M7. `konf-init-kell` banned word + internal vocabulary
**Files**: `crates/konf-init-kell/Cargo.toml`, `crates/konf-init-kell/src/main.rs`, possibly a crate directory rename.
**Fix**: depends on **Q3** (Q3 means the third question, which is the M7 question here — see Q3 in the decisions section).

- **Q3(a)** (recommended): rename crate directory to
  `crates/konf-scaffold`, update `Cargo.toml` `name` and `description`,
  rewrite `main.rs` variables (`kell_dir` → `product_dir`), update the
  workspace `[workspace]` members if it lists the crate by full path
  (it uses glob `crates/*` plus one explicit `crates/konf-init-kell`
  line — drop the explicit line after the glob picks up the renamed
  crate). Update MENTAL_MODEL to remove the vestigial-name exception.
- **Q3(b)**: rename to `konf-new` (matching `cargo new`). Same work.
- **Q3(c)**: keep crate name, fix description + internal strings.
  Touches fewer files.
- **Q3(d)**: delete the crate. Check for any referrer first.

**Decision required**: **yes, Q3** (the crate rename question).

---

### LOW

#### L1. `konf-tool-runner` lib.rs claims about API surface
**Files**: `crates/konf-tool-runner/src/lib.rs:5-38`
**Fix**: Rewrite the module-level doc to tell the truth:
- The **four tools** are the primary API.
- The `Runner` trait is **re-exported** so external crates can write
  new backends (systemd, docker, ssh, …); adding a new backend means
  `impl Runner for MyBackend { … }` and calling `runner::register` with it.
- The `RunRegistry` is also re-exported because multi-backend
  deployments will share one registry across backends.

Also update the README to match.

**Decision required**: no.

#### L2. Broken cross-ref to finding 017
**Files**: `products/ci/README.md:52`
**Fix**: Change `017-runner-tool-not-kernel.md` →
`017-runner-is-a-tool-family.md`. One-character fix. Also grep for any
other `017-runner-*` references in the repo.

**Decision required**: no.

#### L3. Registry unbounded growth
**Files**: `crates/konf-tool-runner/src/registry.rs`
**Fix**: depends on **Q4**. Sketch for **Q4(a)**:

- Add a `size_bound: usize` field to `RunRegistry` (default 1000) set
  at `new()` time.
- Store a companion `VecDeque<RunId>` of terminal run ids in the order
  they completed, under a mutex.
- When a run reaches a terminal state (`mark_terminal`), push the id
  onto the back of the VecDeque; if the deque exceeds `size_bound`,
  pop from the front and remove that id from the papaya map.
- Document the policy in the `RunRegistry` doc comment.

Add a test that spawns 1500 trivial runs and asserts the registry size
stabilizes at 1000 and that the first 500 run ids return "not found"
from `status`.

**Decision required**: **yes, Q4**.

#### L4. `created_count` in `add_nodes`
**Files**: `crates/konf-tool-memory-surreal/src/backend.rs`
**Fix**: After H2 (whole-batch transaction), this counter becomes
redundant — either the whole batch succeeds (`created = nodes.len()`)
or the whole batch errors. Replace the counter with `nodes.len()` in
the success path. No loop arithmetic.

**Decision required**: no (subsumed by H2).

---

### NIT

#### N1. Step 7 inline comment mismatch
**Files**: `crates/konf-init/src/lib.rs:129`
**Fix**: Part of the general docstring rewrite in M5 — the numbering
will be regenerated from scratch to match the actual code flow.

**Decision required**: no.

#### N2. `@0@` operator in SurrealDB 3.x
**Files**: `crates/konf-tool-memory-surreal/src/search.rs:86`
**Fix**: Verify via Context7 (`/websites/surrealdb_surrealql`) whether
`@0@ $q` is the correct SurrealDB 3.x syntax for querying a `FULLTEXT
BM25 HIGHLIGHTS` index. If correct, leave as-is and add a comment citing
the doc page. If incorrect, update the query to the correct syntax and
re-run the integration test that currently asserts text search returns
matches — if the test still passes, the old syntax was falling through
to a table scan and masked the issue.

**Decision required**: no (research step, not a decision).

#### N3. `engine.resources()` vs `runtime.engine().resources()`
**Files**: `crates/konf-init/src/lib.rs:207-210`
**Fix**: Change the log line to uniformly use `runtime.engine()` for
both `tools` and `resources`:

```rust
info!(
    tools = runtime.engine().registry().len(),
    resources = runtime.engine().resources().len(),
    ...
);
```

**Decision required**: no.

#### N4. Test assertion too loose
**Files**: `crates/konf-tool-runner/tests/tool_surface.rs:120-135`
**Fix**: Rewrite `wait_unknown_run_errors` to:
- Set `timeout_secs: 30` so a bug that blocks instead of returning
  "not found" would take 30s to hide.
- Assert the error contains "not found" specifically.
- Assert the call completes in under 100ms (catch the "silently waits"
  regression).

**Decision required**: no.

---

## Commit plan (after decisions land)

One commit per area, atomic, conventional messages. Rough order:

1. `fix(runner): close wait_terminal lost-wakeup race` — C1 + N4 regression
   test.
2. `feat(runner): size-bound registry eviction` — L3 (Q4-dependent).
3. `refactor(memory-surreal): drop is_retracted soft-delete` — H1 (Q2-dependent).
4. `fix(memory-surreal): wrap add_nodes in a SurrealDB transaction` — H2 + L4.
5. `fix(memory-surreal): wrap state_set in a SurrealDB transaction` — H3.
6. `fix(memory-surreal): compute TTL expires_at in Rust, not SurrealQL` — M3.
7. `fix(memory-surreal): resolve_namespace falls back to cfg.namespace` — M1.
8. `refactor(memory-surreal): drop resp_count / MergedCount workaround` — M6.
9. `refactor(memory-surreal): delete dead reapply_schema helper` — M4.
10. `fix(search): verify and correct FULLTEXT operator syntax` — N2 (if needed).
11. `docs(runner): rewrite lib.rs module comment to match surface` — M2, L1.
12. `docs(ci): rewrite gate.yaml to match description` — C2 (Q1-dependent).
13. `docs(products): fix 017 cross-reference` — L2.
14. `docs(mental-model): drop Postgres-required claim` — C3.
15. `docs(init): rewrite architecture/init.md from current code` — H4 + H5.
16. `docs(init): rewrite boot() step docstring to match implementation` — M5 + N1.
17. `chore(log): use runtime.engine() consistently in boot complete log` — N3.
18. `refactor(konf-scaffold): rename konf-init-kell → konf-scaffold` — M7 (Q3-dependent).

Each commit ends with: `cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace` green. No exceptions.

## Verification after all commits

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace`
4. `cargo run -p konf-tool-memory-surreal --example experiment` — 14/14 PASS, including subprocess reopen.
5. `rg -n '\.unwrap\(\)|\.expect\(' crates/*/src | grep -v '#\[cfg(test)\]'` — zero hits.
6. Grep every banned word from `docs/MENTAL_MODEL.md` kill list across
   the whole repo — zero hits outside the kill list itself.
7. Re-run the `audit/issues-prd.md` fresh-eyes pass (via a new background
   agent) on the resolved tree. Expected result: zero CRITICAL, zero
   HIGH, ideally zero MEDIUM issues. Any remaining findings get their
   own follow-up.

## Branching

All work on `audit/prd-discovery`. After verification, two options:

- **Option α**: squash-merge `audit/prd-discovery` into `main` as a single
  "audit fixes" commit with a link to `products/audit/issues-prd.md`.
- **Option β**: keep `audit/prd-discovery` as a long-lived branch for the
  opinionated rebuild the user mentioned. `main` stays at the current
  Phase 1–3 state. Rebuild branches off `audit/prd-discovery`.

I expect **β** because the user said "this is an opinionated build, so
will keep current main and this experience separate". Confirm on merge.

---

*Generated 2026-04-11 as companion to `issues-prd.md`.*
