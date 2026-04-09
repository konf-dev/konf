//! Tests for ProcessTable — concurrent data structure for tracking workflow runs.

use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;

use chrono::Utc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use konf_runtime::process::{ActiveNode, NodeStatus, ProcessTable, RunStatus, WorkflowRun};
use konf_runtime::scope::{Actor, ActorRole};

fn make_run(namespace: &str, status: RunStatus, parent_id: Option<Uuid>) -> WorkflowRun {
    WorkflowRun {
        id: Uuid::new_v4(),
        parent_id,
        workflow_id: "test_workflow".into(),
        namespace: namespace.into(),
        actor: Actor {
            id: "test".into(),
            role: ActorRole::User,
        },
        capabilities: vec!["*".into()],
        metadata: HashMap::new(),
        started_at: Utc::now(),
        status: Mutex::new(status),
        completed_at: Mutex::new(None),
        active_nodes: Mutex::new(Vec::new()),
        steps_executed: AtomicUsize::new(0),
        cancel_token: CancellationToken::new(),
    }
}

#[test]
fn test_insert_and_get() {
    let table = ProcessTable::new();
    let run = make_run("konf:test:user_1", RunStatus::Running, None);
    let run_id = run.id;

    table.insert(run);

    let found = table.get(&run_id, |r| r.workflow_id.clone());
    assert_eq!(found, Some("test_workflow".to_string()));
}

#[test]
fn test_get_nonexistent() {
    let table = ProcessTable::new();
    let result = table.get(&Uuid::new_v4(), |_| ());
    assert!(result.is_none());
}

#[test]
fn test_update() {
    let table = ProcessTable::new();
    let run = make_run("konf:test:user_1", RunStatus::Running, None);
    let run_id = run.id;
    table.insert(run);

    let updated = table.update(&run_id, |r| {
        *r.status.lock().unwrap() = RunStatus::Completed {
            duration_ms: 100,
            output: serde_json::Value::Null,
        };
    });
    assert!(updated);

    let status = table.get(&run_id, |r| r.status.lock().unwrap().clone());
    assert!(matches!(
        status,
        Some(RunStatus::Completed {
            duration_ms: 100,
            output: serde_json::Value::Null
        })
    ));
}

#[test]
fn test_update_nonexistent() {
    let table = ProcessTable::new();
    let updated = table.update(&Uuid::new_v4(), |_| {});
    assert!(!updated);
}

#[test]
fn test_remove() {
    let table = ProcessTable::new();
    let run = make_run("konf:test:user_1", RunStatus::Running, None);
    let run_id = run.id;
    table.insert(run);

    assert!(table.remove(&run_id));
    assert!(table.get(&run_id, |_| ()).is_none());
}

#[test]
fn test_remove_nonexistent() {
    let table = ProcessTable::new();
    assert!(!table.remove(&Uuid::new_v4()));
}

#[test]
fn test_list_all() {
    let table = ProcessTable::new();
    table.insert(make_run("konf:a:user_1", RunStatus::Running, None));
    table.insert(make_run("konf:b:user_2", RunStatus::Running, None));

    let runs = table.list(None);
    assert_eq!(runs.len(), 2);
}

#[test]
fn test_list_with_namespace_filter() {
    let table = ProcessTable::new();
    table.insert(make_run("konf:product_a:user_1", RunStatus::Running, None));
    table.insert(make_run("konf:product_a:user_2", RunStatus::Running, None));
    table.insert(make_run("konf:product_b:user_3", RunStatus::Running, None));

    let runs = table.list(Some("konf:product_a"));
    assert_eq!(runs.len(), 2);

    let runs = table.list(Some("konf:product_b"));
    assert_eq!(runs.len(), 1);

    let runs = table.list(Some("konf:nonexistent"));
    assert_eq!(runs.len(), 0);
}

#[test]
fn test_children_of() {
    let table = ProcessTable::new();
    let parent = make_run("konf:test", RunStatus::Running, None);
    let parent_id = parent.id;
    table.insert(parent);

    table.insert(make_run("konf:test", RunStatus::Running, Some(parent_id)));
    table.insert(make_run("konf:test", RunStatus::Running, Some(parent_id)));
    table.insert(make_run("konf:test", RunStatus::Running, None)); // not a child

    let children = table.children_of(parent_id);
    assert_eq!(children.len(), 2);
}

#[test]
fn test_active_count() {
    let table = ProcessTable::new();
    table.insert(make_run("konf:test", RunStatus::Running, None));
    table.insert(make_run("konf:test", RunStatus::Running, None));
    table.insert(make_run(
        "konf:test",
        RunStatus::Completed {
            duration_ms: 100,
            output: serde_json::Value::Null,
        },
        None,
    ));

    assert_eq!(table.active_count(), 2);
}

#[test]
fn test_active_count_in_namespace() {
    let table = ProcessTable::new();
    table.insert(make_run("konf:a:user_1", RunStatus::Running, None));
    table.insert(make_run("konf:a:user_2", RunStatus::Running, None));
    table.insert(make_run("konf:b:user_3", RunStatus::Running, None));

    assert_eq!(table.active_count_in_namespace("konf:a"), 2);
    assert_eq!(table.active_count_in_namespace("konf:b"), 1);
    assert_eq!(table.active_count_in_namespace("konf:c"), 0);
}

#[test]
fn test_gc_removes_old_completed() {
    let table = ProcessTable::new();

    // Running — should survive gc
    table.insert(make_run("konf:test", RunStatus::Running, None));

    // Completed with old timestamp — should be gc'd
    let old_run = make_run(
        "konf:test",
        RunStatus::Completed {
            duration_ms: 100,
            output: serde_json::Value::Null,
        },
        None,
    );
    let old_id = old_run.id;
    *old_run.completed_at.lock().unwrap() = Some(Utc::now() - chrono::Duration::hours(2));
    table.insert(old_run);

    // Completed with recent timestamp — should survive
    let recent_run = make_run(
        "konf:test",
        RunStatus::Completed {
            duration_ms: 200,
            output: serde_json::Value::Null,
        },
        None,
    );
    let recent_id = recent_run.id;
    *recent_run.completed_at.lock().unwrap() = Some(Utc::now());
    table.insert(recent_run);

    table.gc(std::time::Duration::from_secs(3600)); // 1 hour max age

    assert!(
        table.get(&old_id, |_| ()).is_none(),
        "Old completed run should be gc'd"
    );
    assert!(
        table.get(&recent_id, |_| ()).is_some(),
        "Recent completed run should survive"
    );
    assert_eq!(table.active_count(), 1, "Running run should survive");
}

#[test]
fn test_run_status_is_terminal() {
    assert!(!RunStatus::Pending.is_terminal());
    assert!(!RunStatus::Running.is_terminal());
    assert!(RunStatus::Completed {
        duration_ms: 0,
        output: serde_json::Value::Null
    }
    .is_terminal());
    assert!(RunStatus::Failed {
        error: "err".into(),
        duration_ms: 0
    }
    .is_terminal());
    assert!(RunStatus::Cancelled {
        reason: "test".into(),
        duration_ms: 0
    }
    .is_terminal());
}

#[test]
fn test_to_summary() {
    let table = ProcessTable::new();
    let run = make_run("konf:test:user_1", RunStatus::Running, None);
    let run_id = run.id;

    // Add an active node
    run.active_nodes.lock().unwrap().push(ActiveNode {
        node_id: "step_1".into(),
        tool_name: "echo".into(),
        started_at: Utc::now(),
        status: NodeStatus::Running,
    });
    run.steps_executed
        .store(5, std::sync::atomic::Ordering::Relaxed);

    table.insert(run);

    let summary = table.get(&run_id, |r| r.to_summary()).unwrap();
    assert_eq!(summary.workflow_id, "test_workflow");
    assert_eq!(summary.namespace, "konf:test:user_1");
    assert_eq!(summary.active_node_count, 1);
    assert_eq!(summary.steps_executed, 5);
}

#[tokio::test]
async fn test_concurrent_insert_and_read() {
    let table = std::sync::Arc::new(ProcessTable::new());
    let mut handles = Vec::new();

    // 20 concurrent inserts
    for i in 0..20 {
        let t = table.clone();
        handles.push(tokio::spawn(async move {
            let run = make_run(&format!("konf:test:user_{i}"), RunStatus::Running, None);
            t.insert(run);
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    assert_eq!(table.list(None).len(), 20);
    assert_eq!(table.active_count(), 20);
}
