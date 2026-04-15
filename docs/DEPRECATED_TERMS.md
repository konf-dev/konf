# Deprecated terms

Informational only. No linting. If you see these in docs, they are either historical (fine) or should be replaced with the current term on next edit.

## Renamed / retired concepts

| Don't use | Use instead | Why |
|---|---|---|
| `kell` | `product` | Early naming for a product config directory; replaced by "product" in MENTAL_MODEL.md. The `konf-init-kell` crate binary is vestigial; disposition tracked in the substrate rebuild plan. |
| `cell` | `product` | Earlier still; replaced by "product". |
| `PID 1` as doctrine term | "the konf binary booting in a MicroVM" | The PID-1-as-analogy is evocative for MicroVM scenarios but is not a technical claim about the runtime's process model. Use concrete language. |
| `Hovercraft` / `Construct` / `Zion` / `Operator` / `Operatives` | product / runtime / namespace / actor / etc. | Matrix-metaphor tourism. Replace with the actual term. |
| `Broker` as a doctrine term | — | May still appear as a persona prompt file inside a specific product (e.g. `products/my-product/prompts/broker.md`); not a platform concept. |

## Claims that are aspirational or overstated

| Phrase | Status | Better phrasing |
|---|---|---|
| "autonomous agent civilization" | No code implements this. | Describe the specific multi-agent workflow you mean, with citations. |
| "self-modification" / "self-healing bureaucracy" | No code implements this. | Describe the reload-on-file-change behavior in `konf-init` if that's what's meant. |
| "operational honesty" (as LLM emotional error messages) | No code implements this. | "Errors propagate loudly; runtime events are journaled." |
| "production-grade" / "best industry practices" | Vague. | "Tests pass, clippy clean, no `.unwrap()` in production code, errors propagated via `?`." |

## What we removed and why

The former `MENTAL_MODEL.md` kill list (deleted at stage-0) enforced avoidance of the above terms via a `docs-lint` hook. In practice:

- It banned thinking-words (`first principles`, `curiosity`, `freedom`, `quality`) that are load-bearing for the project's own operating doctrine.
- It could not distinguish hype from precise usage in context.
- It overlapped with the stronger, existing "cite code/findings or mark TBD" rule, which catches the real problem.

The discipline we keep: **every load-bearing claim cites code (file + line), an experimentally verified finding, or an explicit TBD.** That rule is enforced by review, not regex.
