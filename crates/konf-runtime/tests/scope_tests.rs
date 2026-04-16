//! Additional tests for ExecutionScope — validate_start, capability_patterns, depth.

use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;

use chrono::Utc;
use tokio_util::sync::CancellationToken;

use konf_runtime::process::{ProcessTable, RunStatus, WorkflowRun};
use konf_runtime::scope::*;

fn make_running_run(namespace: &str) -> WorkflowRun {
    WorkflowRun {
        id: uuid::Uuid::new_v4(),
        parent_id: None,
        workflow_id: "test".into(),
        namespace: namespace.into(),
        actor: Actor {
            id: "test".into(),
            role: ActorRole::User,
        },
        capabilities: vec![],
        metadata: HashMap::new(),
        started_at: Utc::now(),
        status: Mutex::new(RunStatus::Running),
        completed_at: Mutex::new(None),
        active_nodes: Mutex::new(Vec::new()),
        steps_executed: AtomicUsize::new(0),
        cancel_token: CancellationToken::new(),
    }
}

fn test_scope(namespace: &str) -> ExecutionScope {
    ExecutionScope {
        namespace: namespace.into(),
        capabilities: vec![],
        limits: ResourceLimits::default(),
        actor: Actor {
            id: "user_1".into(),
            role: ActorRole::User,
        },
        depth: 0,
    }
}

#[test]
fn test_validate_start_within_limit() {
    let table = ProcessTable::new();
    table.insert(make_running_run("konf:test:user_1"));

    let scope = ExecutionScope {
        limits: ResourceLimits {
            max_active_runs_per_namespace: 5,
            ..Default::default()
        },
        ..test_scope("konf:test:user_1")
    };

    assert!(scope.validate_start(&table).is_ok());
}

#[test]
fn test_validate_start_at_limit() {
    let table = ProcessTable::new();
    let scope = ExecutionScope {
        limits: ResourceLimits {
            max_active_runs_per_namespace: 2,
            ..Default::default()
        },
        ..test_scope("konf:test")
    };

    table.insert(make_running_run("konf:test:a"));
    table.insert(make_running_run("konf:test:b"));

    let result = scope.validate_start(&table);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("max_active_runs_per_namespace"));
}

#[test]
fn test_validate_start_different_namespace_not_counted() {
    let table = ProcessTable::new();
    table.insert(make_running_run("konf:other:user_1"));
    table.insert(make_running_run("konf:other:user_2"));

    let scope = ExecutionScope {
        limits: ResourceLimits {
            max_active_runs_per_namespace: 1,
            ..Default::default()
        },
        ..test_scope("konf:test")
    };

    assert!(scope.validate_start(&table).is_ok());
}

#[test]
fn test_capability_patterns_extraction() {
    let scope = ExecutionScope {
        capabilities: vec![
            CapabilityGrant::new("memory:*"),
            CapabilityGrant::new("ai:complete"),
            CapabilityGrant::with_bindings("http:get", HashMap::new()),
        ],
        ..test_scope("konf:test")
    };

    let patterns = scope.capability_patterns();
    assert_eq!(patterns, vec!["memory:*", "ai:complete", "http:get"]);
}

#[test]
fn test_capability_patterns_empty() {
    let scope = test_scope("konf:test");
    assert!(scope.capability_patterns().is_empty());
}

#[test]
fn test_child_scope_inherits_limits() {
    let parent = ExecutionScope {
        capabilities: vec![CapabilityGrant::new("*")],
        limits: ResourceLimits {
            max_steps: 500,
            max_workflow_timeout_ms: 60_000,
            ..Default::default()
        },
        actor: Actor {
            id: "admin".into(),
            role: ActorRole::ProductAdmin,
        },
        ..test_scope("konf:test")
    };

    let child = parent
        .child_scope(
            vec![CapabilityGrant::new("memory:search")],
            Some("konf:test:user_1".into()),
        )
        .unwrap();

    assert_eq!(child.limits.max_steps, 500);
    assert_eq!(child.limits.max_workflow_timeout_ms, 60_000);
    assert_eq!(child.namespace, "konf:test:user_1");
    assert_eq!(child.depth, 1); // incremented from parent's 0
}

#[test]
fn test_resource_limits_default_values() {
    let limits = ResourceLimits::default();
    assert_eq!(limits.max_steps, 1000);
    assert_eq!(limits.max_workflow_timeout_ms, 300_000);
    assert_eq!(limits.max_concurrent_nodes, 50);
    assert_eq!(limits.max_child_depth, 10);
    assert_eq!(limits.max_active_runs_per_namespace, 20);
    assert!(limits.validate().is_ok());
}

#[test]
fn test_resource_limits_validation_rejects_zero() {
    let limits = ResourceLimits {
        max_steps: 0,
        ..ResourceLimits::default()
    };
    assert!(limits.validate().is_err());

    let limits = ResourceLimits {
        max_workflow_timeout_ms: 0,
        ..ResourceLimits::default()
    };
    assert!(limits.validate().is_err());

    let limits = ResourceLimits {
        max_active_runs_per_namespace: 0,
        ..ResourceLimits::default()
    };
    assert!(limits.validate().is_err());
}

#[test]
fn test_child_depth_limit_enforced() {
    let table = ProcessTable::new();
    let scope = ExecutionScope {
        limits: ResourceLimits {
            max_child_depth: 3,
            ..Default::default()
        },
        depth: 3, // at the limit
        ..test_scope("konf:test")
    };

    let result = scope.validate_start(&table);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("max_child_depth"));
}

#[test]
fn test_child_depth_within_limit() {
    let table = ProcessTable::new();
    let scope = ExecutionScope {
        limits: ResourceLimits {
            max_child_depth: 5,
            ..Default::default()
        },
        depth: 2, // within limit
        ..test_scope("konf:test")
    };

    assert!(scope.validate_start(&table).is_ok());
}

#[test]
fn test_child_scope_increments_depth() {
    let parent = ExecutionScope {
        capabilities: vec![CapabilityGrant::new("*")],
        depth: 3,
        ..test_scope("konf:test")
    };

    let child = parent
        .child_scope(vec![CapabilityGrant::new("echo")], None)
        .unwrap();
    assert_eq!(child.depth, 4);

    let grandchild = child
        .child_scope(vec![CapabilityGrant::new("echo")], None)
        .unwrap();
    assert_eq!(grandchild.depth, 5);
}
