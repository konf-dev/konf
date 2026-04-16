//! Envelope round-trip and composition tests.
//!
//! Amendment 12: proptest round-trip against postcard encoding prevents
//! format drift from mainlining before tests catch it.

use chrono::Utc;
use proptest::prelude::*;
use serde_json::{json, Value};
use uuid::Uuid;

use konflux_substrate::envelope::*;

// ============================================================
// Proptest strategies for envelope types
// ============================================================

fn arb_uuid() -> impl Strategy<Value = Uuid> {
    any::<[u8; 16]>().prop_map(Uuid::from_bytes)
}

fn arb_envelope_id() -> impl Strategy<Value = EnvelopeId> {
    arb_uuid().prop_map(EnvelopeId)
}

fn arb_trace_id() -> impl Strategy<Value = TraceId> {
    arb_uuid().prop_map(TraceId)
}

fn arb_capability() -> impl Strategy<Value = Capability> {
    "[a-z:*]{1,30}".prop_map(Capability)
}

fn arb_cap_set() -> impl Strategy<Value = CapSet> {
    proptest::collection::vec(arb_capability(), 0..5).prop_map(CapSet)
}

fn arb_metadata() -> impl Strategy<Value = Metadata> {
    proptest::collection::btree_map("[a-z_]{1,15}", arb_json_value(), 0..3).prop_map(Metadata)
}

fn arb_json_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::Number(n.into())),
        "[a-zA-Z0-9 ]{0,50}".prop_map(Value::String),
    ]
}

fn arb_envelope() -> impl Strategy<Value = Envelope<Value>> {
    let identity = (
        arb_envelope_id(),
        arb_trace_id(),
        proptest::option::of(arb_envelope_id()),
        "[a-z_]{1,20}", // actor_id
        "[a-z:]{1,30}", // namespace
        arb_cap_set(),
    );
    let dispatch = (
        "[a-z:]{1,20}",   // target
        "[a-z_]{1,15}",   // payload_type
        arb_json_value(), // payload
        any::<u64>(),     // step_index
        "[a-z_]{1,10}",   // stream_id
        arb_metadata(),
    );
    (identity, dispatch).prop_map(
        |(
            (id, trace_id, parent_id, actor_id, namespace, capabilities),
            (target, payload_type, payload, step_index, stream_id, metadata),
        )| {
            Envelope {
                id,
                trace_id,
                parent_id,
                actor_id: ActorId(actor_id),
                namespace: Namespace(namespace),
                capabilities,
                target: TargetId(target),
                payload_type: PayloadType(payload_type),
                payload,
                emitted_at: Utc::now(),
                step_index,
                stream_id: StreamId(stream_id),
                deadline: None,
                idempotency_key: None,
                references: None,
                metadata,
            }
        },
    )
}

// ============================================================
// Proptest: postcard round-trip
// ============================================================

proptest! {
    #[test]
    fn envelope_postcard_round_trip(env in arb_envelope()) {
        let bytes = postcard::to_allocvec(&env).expect("serialize");
        let decoded: Envelope<Value> = postcard::from_bytes(&bytes).expect("deserialize");

        // Identity fields must survive round-trip exactly.
        prop_assert_eq!(decoded.id, env.id);
        prop_assert_eq!(decoded.trace_id, env.trace_id);
        prop_assert_eq!(decoded.parent_id, env.parent_id);
        prop_assert_eq!(decoded.actor_id, env.actor_id);
        prop_assert_eq!(decoded.namespace, env.namespace);
        prop_assert_eq!(decoded.capabilities, env.capabilities);
        prop_assert_eq!(decoded.target, env.target);
        prop_assert_eq!(decoded.payload_type, env.payload_type);
        prop_assert_eq!(decoded.step_index, env.step_index);
        prop_assert_eq!(decoded.stream_id, env.stream_id);
        prop_assert_eq!(decoded.metadata, env.metadata);
        // Payload round-trips via serde_json::Value equality.
        prop_assert_eq!(decoded.payload, env.payload);
    }

    #[test]
    fn envelope_json_round_trip(env in arb_envelope()) {
        let json = serde_json::to_vec(&env).expect("serialize");
        let decoded: Envelope<Value> = serde_json::from_slice(&json).expect("deserialize");

        prop_assert_eq!(decoded.id, env.id);
        prop_assert_eq!(decoded.trace_id, env.trace_id);
        prop_assert_eq!(decoded.parent_id, env.parent_id);
        prop_assert_eq!(decoded.actor_id, env.actor_id);
        prop_assert_eq!(decoded.namespace, env.namespace);
        prop_assert_eq!(decoded.payload, env.payload);
    }
}

// ============================================================
// Deterministic: respond() propagation
// ============================================================

#[test]
fn respond_propagates_identity_fields() {
    let parent = Envelope::test(json!({"input": "hello"}));
    let child = parent.respond(json!({"output": "world"}));

    // Identity propagation
    assert_eq!(child.trace_id, parent.trace_id, "trace_id inherits");
    assert_eq!(child.parent_id, Some(parent.id), "parent_id links back");
    assert_eq!(child.actor_id, parent.actor_id, "actor_id inherits");
    assert_eq!(child.namespace, parent.namespace, "namespace inherits");
    assert_eq!(
        child.capabilities, parent.capabilities,
        "capabilities inherit"
    );
    assert_eq!(child.stream_id, parent.stream_id, "stream_id inherits");

    // New identity
    assert_ne!(child.id, parent.id, "child gets fresh id");
    assert_eq!(
        child.step_index,
        parent.step_index + 1,
        "step_index increments"
    );

    // Payload replaced
    assert_eq!(child.payload, json!({"output": "world"}));
    assert_ne!(child.payload, parent.payload);

    // Cleared
    assert!(child.idempotency_key.is_none());
    assert!(child.references.is_none());
}

#[test]
fn respond_chain_builds_causal_chain() {
    let root = Envelope::test(json!(1));
    let mid = root.respond(json!(2));
    let leaf = mid.respond(json!(3));

    // All share the same trace
    assert_eq!(root.trace_id, mid.trace_id);
    assert_eq!(mid.trace_id, leaf.trace_id);

    // parent_id chain
    assert_eq!(root.parent_id, None);
    assert_eq!(mid.parent_id, Some(root.id));
    assert_eq!(leaf.parent_id, Some(mid.id));

    // step_index monotonic
    assert_eq!(root.step_index, 0);
    assert_eq!(mid.step_index, 1);
    assert_eq!(leaf.step_index, 2);
}

#[test]
fn test_envelope_builds_minimal_valid_envelope() {
    let env = Envelope::test(json!({"key": "val"}));
    assert_eq!(env.payload, json!({"key": "val"}));
    assert_eq!(env.actor_id, ActorId("test".to_string()));
    assert_eq!(env.namespace, Namespace("test".to_string()));
    assert_eq!(env.step_index, 0);
    assert!(env.parent_id.is_none());
    assert!(env.deadline.is_none());
}

#[test]
fn for_tool_dispatch_maps_fields_correctly() {
    let trace = Uuid::new_v4();
    let env = Envelope::for_tool_dispatch(
        "memory:search",
        json!({"query": "test"}),
        &["memory:*".to_string(), "ai:complete".to_string()],
        trace,
        "konf:test:user1",
        "user_42",
        "session_abc",
    );

    assert_eq!(env.target, TargetId("memory:search".to_string()));
    assert_eq!(env.trace_id, TraceId(trace));
    assert_eq!(env.namespace, Namespace("konf:test:user1".to_string()));
    assert_eq!(env.actor_id, ActorId("user_42".to_string()));
    assert_eq!(env.stream_id, StreamId("session_abc".to_string()));
    assert_eq!(env.capabilities.0.len(), 2);
    assert_eq!(env.capabilities.0[0], Capability("memory:*".to_string()));
    assert_eq!(env.payload, json!({"query": "test"}));
}
