//! Smoke test — `konf-genesis/products/orchestrator/` parses under konf-init.
//!
//! Validates that the YAML files authored for the Stigmergic Engine
//! orchestrator product can be loaded by the konf-init boot pipeline.
//! This is not a full end-to-end test (that would require a running
//! backend + live MCP clients — see `konf-genesis/tests/smoke_orchestrator.sh`
//! for the shell-driven E2E path).
//!
//! What this test covers:
//! - `project.yaml`, `tools.yaml` parse as the right shape
//! - All three workflow YAMLs parse and declare the expected tools
//! - Prompt files exist
//!
//! What this test does NOT cover (deferred to Phase G E2E):
//! - Worktree sandbox enforcement (needs live git + shell)
//! - Trace resurfacing (needs running SurrealDB with interaction writes)
//! - MCP session propagation (needs live MCP clients)

use std::fs;
use std::path::{Path, PathBuf};

fn orchestrator_config_dir() -> PathBuf {
    // The konf-genesis repo sits at workspace-root/../konf-genesis.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent() // crates/
        .and_then(|p| p.parent()) // konf/
        .and_then(|p| p.parent()) // workspace root
        .expect("resolve workspace root")
        .join("konf-genesis/products/orchestrator/config")
}

#[test]
fn orchestrator_product_dir_exists() {
    let dir = orchestrator_config_dir();
    assert!(
        dir.exists(),
        "orchestrator config dir missing at {}",
        dir.display()
    );
    assert!(dir.join("project.yaml").exists());
    assert!(dir.join("tools.yaml").exists());
}

#[test]
fn all_three_workflow_yamls_exist() {
    let wf = orchestrator_config_dir().join("workflows");
    for name in ["query_view.yaml", "spawn_in_worktree.yaml", "summarize_subtree.yaml"] {
        let path = wf.join(name);
        assert!(path.exists(), "missing workflow: {}", path.display());
    }
}

#[test]
fn project_yaml_parses_as_yaml() {
    let path = orchestrator_config_dir().join("project.yaml");
    let content = fs::read_to_string(&path).expect("read project.yaml");
    let _parsed: serde_yaml::Value =
        serde_yaml::from_str(&content).expect("project.yaml must be valid YAML");
}

#[test]
fn tools_yaml_parses_and_has_memory_backend() {
    let path = orchestrator_config_dir().join("tools.yaml");
    let content = fs::read_to_string(&path).expect("read tools.yaml");
    let parsed: serde_yaml::Value =
        serde_yaml::from_str(&content).expect("tools.yaml must be valid YAML");
    let memory = parsed
        .get("tools")
        .and_then(|t| t.get("memory"))
        .expect("tools.memory section required");
    assert_eq!(
        memory.get("backend").and_then(|v| v.as_str()),
        Some("surreal"),
        "orchestrator pins to surreal backend for fan-out journal wiring"
    );
}

#[test]
fn every_workflow_declares_register_as_tool() {
    // Orchestrator workflows must be tool-callable so spawned sub-agents
    // can invoke them over MCP without re-implementing entry points.
    let wf = orchestrator_config_dir().join("workflows");
    for name in ["query_view.yaml", "spawn_in_worktree.yaml", "summarize_subtree.yaml"] {
        let content = fs::read_to_string(wf.join(name)).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
        assert_eq!(
            parsed.get("register_as_tool").and_then(|v| v.as_bool()),
            Some(true),
            "{name} must declare register_as_tool: true"
        );
    }
}

#[test]
fn prompt_files_exist() {
    let prompts = orchestrator_config_dir()
        .parent()
        .unwrap()
        .join("prompts");
    assert!(prompts.join("bird_eye.md").exists());
    assert!(prompts.join("worktree_actor.md").exists());
}
