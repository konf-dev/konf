//! B3 — Automatic capture tests (Phase B).
//!
//! Validates that every `Runtime::invoke_tool` dispatch produces an
//! Interaction-shaped `JournalEntry` in the configured journal — without
//! any cooperation from the tool or its caller. This is the
//! "substrate-enforced tracing" contract.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use konflux::engine::Engine;
use konflux::error::ToolError;
use konflux::tool::{Tool, ToolContext, ToolInfo};
use konf_runtime::interaction::{Interaction, InteractionKind, InteractionStatus};
use konf_runtime::journal::JournalStore;
use konf_runtime::scope::{
    Actor, ActorRole, CapabilityGrant, ExecutionScope, ResourceLimits,
};
use konf_runtime::{JournalEntry, JournalError, JournalRow, RunId, Runtime};
use serde_json::{json, Value};
use uuid::Uuid;

/// Mock journal that captures every append. Can be configured to fail on
/// append to prove failure isolation at the dispatch layer.
#[derive(Default)]
struct CaptureJournal {
    entries: Mutex<Vec<JournalEntry>>,
}

impl CaptureJournal {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Number of appended entries, for polling synchronization.
    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    fn entries(&self) -> Vec<JournalEntry> {
        self.entries.lock().unwrap().clone()
    }
}

#[async_trait]
impl JournalStore for CaptureJournal {
    async fn append(&self, entry: JournalEntry) -> Result<u64, JournalError> {
        let mut v = self.entries.lock().unwrap();
        let id = v.len() as u64;
        v.push(entry);
        Ok(id)
    }
    async fn query_by_run(&self, _: RunId) -> Result<Vec<JournalRow>, JournalError> {
        Ok(vec![])
    }
    async fn query_by_session(
        &self,
        _: &str,
        _: usize,
    ) -> Result<Vec<JournalRow>, JournalError> {
        Ok(vec![])
    }
    async fn recent(&self, _: usize) -> Result<Vec<JournalRow>, JournalError> {
        Ok(vec![])
    }
    async fn reconcile_zombies(&self) -> Result<u64, JournalError> {
        Ok(0)
    }
}

/// A tool that never says anything — proves capture is substrate-enforced.
struct SilentTool;
#[async_trait]
impl Tool for SilentTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "silent".into(),
            description: "does nothing quietly".into(),
            input_schema: json!({}),
            capabilities: vec![],
            supports_streaming: false,
            output_schema: None,
            annotations: Default::default(),
        }
    }
    async fn invoke(&self, _input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        Ok(json!(null))
    }
}

/// A tool that deliberately errors.
struct BrokenTool;
#[async_trait]
impl Tool for BrokenTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "broken".into(),
            description: "always fails".into(),
            input_schema: json!({}),
            capabilities: vec![],
            supports_streaming: false,
            output_schema: None,
            annotations: Default::default(),
        }
    }
    async fn invoke(&self, _input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        Err(ToolError::ExecutionFailed {
            message: "explicit failure".into(),
            retryable: false,
        })
    }
}

async fn runtime_with_journal(journal: Arc<dyn JournalStore>) -> Runtime {
    let engine = Engine::new();
    engine.register_tool(Arc::new(SilentTool));
    engine.register_tool(Arc::new(BrokenTool));
    Runtime::new(engine, Some(journal)).await.unwrap()
}

fn test_scope(namespace: &str, actor_id: &str) -> ExecutionScope {
    ExecutionScope {
        namespace: namespace.into(),
        capabilities: vec![CapabilityGrant::new("*")],
        limits: ResourceLimits::default(),
        actor: Actor {
            id: actor_id.into(),
            role: ActorRole::User,
        },
        depth: 0,
    }
}

/// Wait until `journal.len() >= expected` or timeout. Returns whether the
/// condition became true.
async fn wait_for_entries(journal: &CaptureJournal, expected: usize) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    while tokio::time::Instant::now() < deadline {
        if journal.len() >= expected {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    journal.len() >= expected
}

fn extract_interaction(entry: &JournalEntry) -> Interaction {
    assert_eq!(entry.event_type, "interaction", "expected interaction entry");
    Interaction::from_json(entry.payload.clone()).expect("deserialize interaction")
}

#[tokio::test]
async fn invoke_tool_produces_tool_dispatch_interaction_in_journal() {
    let journal = CaptureJournal::new();
    let runtime = runtime_with_journal(journal.clone()).await;
    let scope = test_scope("konf:test:ns", "alice");
    let exec_ctx = konf_runtime::ExecutionContext::new_root("sess-test");

    runtime
        .invoke_tool("silent", json!({}), &scope, &exec_ctx)
        .await
        .unwrap();

    assert!(wait_for_entries(&journal, 1).await, "journal must receive 1 entry");
    let entries = journal.entries();
    let interaction = extract_interaction(&entries[0]);

    assert_eq!(interaction.kind, InteractionKind::ToolDispatch);
    assert_eq!(interaction.target, "tool:silent");
    assert_eq!(interaction.status, InteractionStatus::Ok);
}

#[tokio::test]
async fn invoke_tool_with_silent_actor_still_records() {
    // Silent = the tool does nothing and returns null. Substrate still records.
    let journal = CaptureJournal::new();
    let runtime = runtime_with_journal(journal.clone()).await;
    let scope = test_scope("konf:test:ns", "alice");
    let exec_ctx = konf_runtime::ExecutionContext::new_root("sess-test");

    runtime.invoke_tool("silent", json!({}), &scope, &exec_ctx).await.unwrap();

    assert!(wait_for_entries(&journal, 1).await);
    let entries = journal.entries();
    let interaction = extract_interaction(&entries[0]);
    assert_eq!(interaction.target, "tool:silent");
}

#[tokio::test]
async fn invoke_tool_producing_error_records_failed_status() {
    let journal = CaptureJournal::new();
    let runtime = runtime_with_journal(journal.clone()).await;
    let scope = test_scope("konf:test:ns", "alice");
    let exec_ctx = konf_runtime::ExecutionContext::new_root("sess-test");

    let result = runtime.invoke_tool("broken", json!({}), &scope, &exec_ctx).await;
    assert!(result.is_err(), "broken tool must surface error");

    assert!(wait_for_entries(&journal, 1).await);
    let entries = journal.entries();
    let interaction = extract_interaction(&entries[0]);
    match interaction.status {
        InteractionStatus::Failed { error } => {
            assert!(
                error.contains("explicit failure"),
                "error text preserved: {error}"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    // Target still recorded.
    assert_eq!(interaction.target, "tool:broken");
}

#[tokio::test]
async fn invoke_tool_records_edge_rules_fired_list() {
    let journal = CaptureJournal::new();
    let runtime = runtime_with_journal(journal.clone()).await;
    let scope = test_scope("konf:test:ns", "alice");
    let exec_ctx = konf_runtime::ExecutionContext::new_root("sess-test");

    runtime.invoke_tool("silent", json!({}), &scope, &exec_ctx).await.unwrap();

    assert!(wait_for_entries(&journal, 1).await);
    let entries = journal.entries();
    let interaction = extract_interaction(&entries[0]);

    assert!(
        interaction
            .edge_rules_fired
            .iter()
            .any(|r| r.starts_with("cap_check:")),
        "must record capability check: {:?}",
        interaction.edge_rules_fired
    );
}

#[tokio::test]
async fn invoke_tool_records_actor_and_namespace_inline() {
    let journal = CaptureJournal::new();
    let runtime = runtime_with_journal(journal.clone()).await;
    let scope = test_scope("konf:unspool:user_99", "user_99");
    let exec_ctx = konf_runtime::ExecutionContext::new_root("sess-test");

    runtime.invoke_tool("silent", json!({}), &scope, &exec_ctx).await.unwrap();

    assert!(wait_for_entries(&journal, 1).await);
    let entries = journal.entries();
    let interaction = extract_interaction(&entries[0]);

    assert_eq!(interaction.namespace, "konf:unspool:user_99");
    assert_eq!(interaction.actor.id, "user_99");
    assert_eq!(interaction.actor.role, ActorRole::User);
}

#[tokio::test]
async fn invoke_tool_preserves_context_trace_id() {
    // R2: trace_id now lives on ExecutionContext (not scope). When the
    // context carries a specific trace, the recorded interaction carries
    // it — no per-call minting.
    let journal = CaptureJournal::new();
    let runtime = runtime_with_journal(journal.clone()).await;
    let trace = Uuid::new_v4();
    let scope = test_scope("konf:test:ns", "alice");
    let exec_ctx = konf_runtime::ExecutionContext::with_trace(trace, "sess-test");

    runtime.invoke_tool("silent", json!({}), &scope, &exec_ctx).await.unwrap();

    assert!(wait_for_entries(&journal, 1).await);
    let interaction = extract_interaction(&journal.entries()[0]);
    assert_eq!(interaction.trace_id, trace);
}

#[tokio::test]
async fn consecutive_invoke_tool_calls_share_trace_id_via_context() {
    // Regression for the C3 bug: two consecutive calls on the same context
    // produce Interactions with the same trace_id. Under the old scope-
    // mints-per-call behavior this test would have FAILED because each
    // call saw scope.trace_id == None and minted its own.
    let journal = CaptureJournal::new();
    let runtime = runtime_with_journal(journal.clone()).await;
    let scope = test_scope("konf:test:ns", "alice");
    let exec_ctx = konf_runtime::ExecutionContext::new_root("sess-test");

    runtime.invoke_tool("silent", json!({}), &scope, &exec_ctx).await.unwrap();
    runtime.invoke_tool("silent", json!({}), &scope, &exec_ctx).await.unwrap();

    assert!(wait_for_entries(&journal, 2).await);
    let entries = journal.entries();
    let ia = extract_interaction(&entries[0]);
    let ib = extract_interaction(&entries[1]);
    assert_eq!(ia.trace_id, ib.trace_id, "C3: consecutive calls share trace_id");
    assert_eq!(ia.trace_id, exec_ctx.trace_id);
}
