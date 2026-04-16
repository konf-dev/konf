//! Scope-to-Envelope composition regression test (4.g).
//!
//! Validates that `ExecutionScope::to_envelope()` correctly maps
//! scope identity fields into typed Envelope fields.

use konf_runtime::scope::{Actor, ActorRole, CapabilityGrant, ExecutionScope, ResourceLimits};
use konflux_substrate::envelope::{
    ActorId, CapSet, Capability, Namespace, StreamId, TargetId, TraceId,
};
use serde_json::json;
use uuid::Uuid;

fn test_scope() -> ExecutionScope {
    ExecutionScope {
        namespace: "konf:product:user_42".into(),
        capabilities: vec![
            CapabilityGrant::new("memory:*"),
            CapabilityGrant::new("ai:complete"),
        ],
        limits: ResourceLimits::default(),
        actor: Actor {
            id: "user_42".into(),
            role: ActorRole::User,
        },
        depth: 0,
    }
}

#[test]
fn scope_to_envelope_preserves_namespace() {
    let scope = test_scope();
    let trace = Uuid::new_v4();
    let env = scope.to_envelope("memory:search", json!({}), trace, "sess_1");

    assert_eq!(env.namespace, Namespace("konf:product:user_42".to_string()));
}

#[test]
fn scope_to_envelope_preserves_actor_id() {
    let scope = test_scope();
    let trace = Uuid::new_v4();
    let env = scope.to_envelope("memory:search", json!({}), trace, "sess_1");

    assert_eq!(env.actor_id, ActorId("user_42".to_string()));
}

#[test]
fn scope_to_envelope_preserves_capabilities() {
    let scope = test_scope();
    let trace = Uuid::new_v4();
    let env = scope.to_envelope("memory:search", json!({}), trace, "sess_1");

    assert_eq!(
        env.capabilities,
        CapSet(vec![
            Capability("memory:*".to_string()),
            Capability("ai:complete".to_string()),
        ])
    );
}

#[test]
fn scope_to_envelope_preserves_trace_id() {
    let scope = test_scope();
    let trace = Uuid::new_v4();
    let env = scope.to_envelope("memory:search", json!({}), trace, "sess_1");

    assert_eq!(env.trace_id, TraceId(trace));
}

#[test]
fn scope_to_envelope_sets_target_and_stream() {
    let scope = test_scope();
    let trace = Uuid::new_v4();
    let env = scope.to_envelope("ai:complete", json!({"prompt": "hi"}), trace, "sess_abc");

    assert_eq!(env.target, TargetId("ai:complete".to_string()));
    assert_eq!(env.stream_id, StreamId("sess_abc".to_string()));
    assert_eq!(env.payload, json!({"prompt": "hi"}));
}

#[test]
fn scope_to_envelope_child_attenuates_capabilities() {
    let parent = test_scope();
    let child_scope = parent
        .child_scope(
            vec![CapabilityGrant::new("memory:search")],
            Some("konf:product:user_42:child".into()),
        )
        .expect("child scope should succeed");

    let trace = Uuid::new_v4();
    let env = child_scope.to_envelope("memory:search", json!({}), trace, "sess_1");

    // Child envelope should have attenuated capabilities
    assert_eq!(
        env.capabilities,
        CapSet(vec![Capability("memory:search".to_string())])
    );
    assert_eq!(
        env.namespace,
        Namespace("konf:product:user_42:child".to_string())
    );
}
