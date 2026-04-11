# konf products/ci/

A konf product whose only job is to run cargo against a konf workspace and
return structured results.

## What it is

Four workflows:

| Workflow            | What it does                                            |
|---------------------|---------------------------------------------------------|
| `cargo_fmt_check`   | `cargo fmt --all -- --check` across the workspace       |
| `cargo_clippy`      | `cargo clippy --workspace --all-targets -- -D warnings` |
| `cargo_test_crate`  | `cargo test -p <crate>` for a single package            |
| `gate`              | Spawn `cargo_fmt_check` and `cargo_clippy` in parallel via `runner:spawn`, wait for both, return the combined result |

All four are `register_as_tool: true`, so they appear as MCP tools when
`konf-mcp` is booted against this product's config directory. A Claude
Code sub-agent (or any MCP client) calling `workflow:gate` drives the
whole ship gate without running `Bash` itself — `shell:exec` runs inside
konf's own process in `host` mode.

## Why this product exists

This is the dogfood demo for Phase 3 of plan `serene-tumbling-gizmo`: it
proves that konf's runner tool family (`runner:spawn`/`status`/`wait`/
`cancel`, added in the `konf-tool-runner` crate) works end-to-end and
gives workflow authors a clean primitive for fan-out. The `gate`
workflow specifically demonstrates composition: it's a parent workflow
that spawns child workflows via `runner:spawn`, waits for them to
finish, and aggregates the results.

## Running it

```bash
# Point konf-mcp at this product's config
konf-mcp --config products/ci/config

# From an MCP client: call `workflow:gate` with a list of crates
```

`KONF_WORKSPACE` must be set to the directory containing `konf/`. The
default cargo timeout is ten minutes (`shell.timeout_ms: 600000` in
`tools.yaml`); increase it in `tools.yaml` if your workspace takes
longer to test.

## Future backends

Today every run uses the inline runner — child workflows execute as
tokio tasks in the same process as `konf-mcp`. When systemd or docker
runner backends land (see `konf-experiments/findings/017-runner-tool-not-kernel.md`
for the decision rationale), gate workflows can opt into per-run
isolation by passing a `runner` field to `runner:spawn`. The `gate`
workflow YAML will not need to change.
