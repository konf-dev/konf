# Operational Process

These rules govern how the Genesis Kell and its agents (human or AI) evolve the company state.

## Core Rules

1.  **Documentation-First:** Write the specification or architecture document before writing any YAML configuration or tool code.
2.  **Verify Before Acting:** Research the current state, validate assumptions with tools, and verify behavior with tests before finalizing changes.
3.  **Surgical Changes:** Focus changes on the specific task. Avoid unrelated refactoring or "cleanup" during a feature implementation.
4.  **Loud Error Handling:** Never fail silently. Code and workflows must provide transparent, actionable error messages.
5.  **Quality over Speed:** There are no artificial timelines. The goal is the best possible product.

## Git Workflow

-   **Everything is Tracked:** All changes to configuration, workflows, and prompts must be committed to the repository.
-   **Atomic Commits:** Each commit should represent a single logical change or experimental step.
-   **Descriptive Messages:** Focus on the "Why" more than the "What."

## Security Protocols

-   **Credential Protection:** Never commit API keys, tokens, or secrets. Use `${VAR}` interpolation.
-   **Zero Disk Exposure:** Production secrets live only in the Infisical Vault and the memory of the running process.
-   **The Membrane:** Execute all external commands within the `konf-sandbox` container unless explicitly required otherwise by an administrator.
