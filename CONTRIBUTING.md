# Contributing to Konf

Thank you for your interest in contributing to Konf!

## How You Can Contribute

| Contribution | Where | Guide |
|-------------|-------|-------|
| **Infrastructure** (Rust crates) | `crates/` | [Architecture docs](docs/architecture/overview.md) |
| **Products** (reference configs) | `products/` | [Product Guide](docs/product-guide/creating-a-product.md) |
| **Documentation** | `docs/` | [Docs index](docs/README.md) |
| **Plugins** (WASM, future) | `sdk/` | [SDK README](sdk/README.md) |

## Terminology

See [`docs/MENTAL_MODEL.md`](docs/MENTAL_MODEL.md) for the full vocabulary, and
[`docs/DEPRECATED_TERMS.md`](docs/DEPRECATED_TERMS.md) for renamed / retired
concepts. The core term is:

- **`product`** — a directory of YAML + markdown defining one konf agent. The
  Rust type is `ProductConfig` (in `crates/konf-init/src/config.rs`).

Any new analogy, metaphor, or vocabulary term introduced to the docs must
cash out to code or an experimentally verified finding. If it doesn't, it
doesn't belong in the docs.

## Reporting Bugs

Open a [bug report](https://github.com/konf-dev/konf/issues/new?template=bug.yml) with steps to reproduce.

## Suggesting Features

Open a [feature request](https://github.com/konf-dev/konf/issues/new?template=feature.yml) describing the problem and proposed solution.

## Code Contributions

### Setup

```bash
git clone https://github.com/konf-dev/konf.git
cd konf
cargo build --workspace
cargo test --workspace
```

### Code Style

- Run `cargo fmt --all` before committing
- Run `cargo clippy --workspace -- -D warnings` — all warnings are errors
- Follow existing patterns in the codebase

### Pull Request Process

1. Fork the repository
2. Create a feature branch (`git checkout -b feat/my-feature`)
3. Write tests for new functionality
4. Ensure all checks pass:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace -- -D warnings
   cargo test --workspace
   ```
5. Submit a pull request

### Commit Messages

Use [conventional commits](https://www.conventionalcommits.org/):
- `feat: add new tool` — new feature
- `fix: resolve SSRF bypass` — bug fix
- `docs: update architecture spec` — documentation
- `refactor: extract tool registry` — code improvement
- `test: add memory backend tests` — test additions

## Product Contributions

Products are pure configuration — no Rust code needed. To contribute a reference product:

1. Copy `products/_template/` to `products/your-product/`
2. Define your tools, workflows, and prompts
3. Add a README explaining what the product does
4. Submit a pull request

## MSRV

Minimum supported Rust version: **latest stable**. We track stable releases.

## License

By contributing to Konf, you agree that your contributions will be licensed under the [Business Source License 1.1](LICENSE).
