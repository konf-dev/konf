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
    "[a-z:*]{1,30}".prop_map(Capability::new)
}

fn arb_cap_set() -> impl Strategy<Value = CapSet> {
    proptest::collection::vec(arb_capability(), 0..5).prop_map(CapSet::from_capabilities)
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
                qos_class: None,
                references: None,
                metadata,
            }
        },
    )
}

// ============================================================
// Proptest: postcard round-trip
// ============================================================

/// Strategy for Envelope<String> — postcard-compatible payload.
fn arb_envelope_string() -> impl Strategy<Value = Envelope<String>> {
    let identity = (
        arb_envelope_id(),
        arb_trace_id(),
        proptest::option::of(arb_envelope_id()),
        "[a-z_]{1,20}", // actor_id
        "[a-z:]{1,30}", // namespace
        arb_cap_set(),
    );
    let dispatch = (
        "[a-z:]{1,20}",       // target
        "[a-z_]{1,15}",       // payload_type
        "[a-zA-Z0-9 ]{0,50}", // payload (String)
        any::<u64>(),         // step_index
        "[a-z_]{1,10}",       // stream_id
    );
    (identity, dispatch).prop_map(
        |(
            (id, trace_id, parent_id, actor_id, namespace, capabilities),
            (target, payload_type, payload, step_index, stream_id),
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
                qos_class: None,
                references: None,
                metadata: Metadata::default(),
            }
        },
    )
}

proptest! {
    /// Postcard round-trip for envelope structure. Uses `Envelope<String>`
    /// because `serde_json::Value` is self-describing and not supported by
    /// postcard. The redb journal handles this by JSON-stringifying the
    /// payload before postcard encoding — same pattern.
    #[test]
    fn envelope_postcard_round_trip(env in arb_envelope_string()) {
        let bytes = postcard::to_allocvec(&env).expect("serialize");
        let decoded: Envelope<String> = postcard::from_bytes(&bytes).expect("deserialize");

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
        prop_assert_eq!(decoded.payload, env.payload);
    }

    /// JSON round-trip for Envelope<Value> — the full wire format.
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
    assert_eq!(env.capabilities.len(), 2);
    assert_eq!(
        env.capabilities.iter().next().unwrap(),
        &Capability::new("memory:*")
    );
    assert_eq!(env.payload, json!({"query": "test"}));
}

// ============================================================
// Stage 6: Capability attenuation
// ============================================================

#[test]
fn capability_matches_exact() {
    let cap = Capability::new("memory:search");
    assert!(cap.matches("memory:search"));
    assert!(!cap.matches("memory:store"));
    assert!(!cap.matches("memory:search:nested"));
}

#[test]
fn capability_matches_prefix() {
    let cap = Capability::new("memory:*");
    assert!(cap.matches("memory:search"));
    assert!(cap.matches("memory:store"));
    assert!(!cap.matches("memorysearch")); // no colon separator
}

#[test]
fn capability_matches_wildcard() {
    let cap = Capability::new("*");
    assert!(cap.matches("anything"));
    assert!(cap.matches("memory:search"));
}

#[test]
fn capset_check_access_empty_denies() {
    let caps = CapSet::default();
    assert!(caps.check_access("echo").is_err());
}

#[test]
fn capset_check_access_grants() {
    let caps = CapSet::from_patterns(&["memory:*", "ai:complete"]);
    assert!(caps.check_access("memory:search").is_ok());
    assert!(caps.check_access("ai:complete").is_ok());
    assert!(caps.check_access("http:get").is_err());
}

#[test]
fn capset_attenuate_subset_ok() {
    let parent = CapSet::from_patterns(&["memory:*", "ai:complete"]);
    let child = parent.attenuate(&["memory:search"]).unwrap();
    assert_eq!(child.len(), 1);
    assert!(child.check_access("memory:search").is_ok());
    assert!(child.check_access("memory:store").is_err());
}

#[test]
fn capset_attenuate_rejects_amplification() {
    let parent = CapSet::from_patterns(&["memory:search"]);
    assert!(parent.attenuate(&["memory:*"]).is_err());
    assert!(parent.attenuate(&["*"]).is_err());
    assert!(parent.attenuate(&["http:get"]).is_err());
}

#[test]
fn capset_attenuate_empty_child_ok() {
    let parent = CapSet::from_patterns(&["*"]);
    let child = parent.attenuate(&Vec::<String>::new()).unwrap();
    assert!(child.is_empty());
}

proptest! {
    /// Property: attenuated CapSet never grants access beyond the parent.
    #[test]
    fn attenuation_is_narrowing_only(
        parent_pats in proptest::collection::vec("[a-z]{1,5}:[a-z]{1,5}", 1..5),
        child_idx in proptest::collection::vec(any::<proptest::sample::Index>(), 0..3),
    ) {
        let parent = CapSet::from_patterns(&parent_pats);

        // Pick a subset of parent patterns for the child.
        let child_pats: Vec<&str> = child_idx.iter()
            .map(|idx| parent_pats[idx.index(parent_pats.len())].as_str())
            .collect();

        let child = parent.attenuate(&child_pats).expect("subset should succeed");

        // For every tool the child grants, the parent must also grant it.
        for pat in child.patterns() {
            prop_assert!(
                parent.check_access(pat).is_ok(),
                "child grants '{pat}' but parent does not"
            );
        }
    }
}
