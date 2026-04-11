# devkit doctrine

This product follows the three rules in `konf/docs/MENTAL_MODEL.md`
(Doctrine section):

1. **The Rust crates are the kernel; YAML + markdown are the configuration.**
   New Rust must be impossible to express as a workflow using existing tools.
2. **Prompts and configs replace code where possible.** If a decision, rule,
   or state change can be prompt + LLM + filesystem, don't write Rust.
3. **Products are configurations, not code.** To ship: write YAML. To update:
   edit YAML. No per-product Rust.

## What devkit is

devkit is the canonical reference product for konf development workflows.
Its workflows validate VCS agent identity (`konf-agents[bot]`), agentic
tool-calling, nested workflow composition, and AI-generated workflows that
pass `yaml:validate_workflow` before committing.

## Rules for adding or editing workflows in this product

- Use **colon form** in `do:` fields: `do: shell:exec`, `do: ai:complete`,
  `do: workflow:run_tests`, `do: config:reload`, `do: yaml:validate_workflow`.
- **MCP-forwarded tools keep their native form**: `do: github:push_files`,
  `do: github:create_branch`.
- **Bare builtins use no namespace**: `do: echo`, `do: template`, `do: log`,
  `do: json_get`, `do: concat`.
- **Every workflow must boot end-to-end before being committed.** Validate
  with `yaml:validate_workflow` and then actually run it through the engine.
  Never ship YAML that hasn't been executed.

## Product authoring reference

- `konf/docs/product-guide/creating-a-product.md` — full product authoring guide
- `konf/docs/product-guide/workflow-reference.md` — workflow YAML schema
- `konf/docs/architecture/tools.md` — full tool catalog with input schemas
