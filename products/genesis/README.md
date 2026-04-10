# Genesis Cell (Reference Implementation)

**This is a read-only snapshot of the Genesis Cell.**

The "living" version of Genesis—The Operator—has been moved to its own standalone repository to separate operational history from the kernel logic.

**Living Repository:** [https://github.com/konf-dev/konf-genesis.kell](https://github.com/konf-dev/konf-genesis.kell)

## Why a Standalone Repo?

Genesis acts as the "Operator" for a production cell. It generates logs, modifies its own workflows, and commits its "thoughts" daily. Keeping this operational history in a separate repository ensures:

1.  **Kernel Purity:** The core Rust kernel repository stays clean of agent-specific operational noise.
2.  **Security Boundaries:** Genesis has push access to its own configuration Cell, but not to the core platform code.
3.  **Cell Architecture:** The standalone repo demonstrates how to organize multiple minimal, composable agents (Kells) within a single deployment unit.

## Architecture

This snapshot reflects the "Cell" structure:
- `kells/operator`: The primary root agent.
- `docker-compose.yml`: The Podman manifest for production deployment.

For the most up-to-date DNA and behavior, refer to the [konf-genesis.kell](https://github.com/konf-dev/konf-genesis.kell) repository.
