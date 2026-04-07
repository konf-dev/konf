# Contributing to Konf

Thank you for your interest in contributing to Konf!

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

## MSRV

Minimum supported Rust version: **latest stable**. We track stable releases.

## License

By contributing to Konf, you agree that your contributions will be licensed under the [Business Source License 1.1](LICENSE).
