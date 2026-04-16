//! R2 — `ExecutionContext` trace_id propagation tests.
//!
//! Replaces the earlier B5 tests that asserted `ExecutionScope::trace_id`
//! behavior. After R2, `trace_id` lives on `ExecutionContext` (runtime
//! state), not on `ExecutionScope` (config). The same invariants hold
//! (root mint, child inheritance, no upward leak) but they're now
//! enforced at the correct boundary.

use konf_runtime::ExecutionContext;
use uuid::Uuid;

#[test]
fn new_root_mints_a_fresh_non_nil_trace_id() {
    let ctx = ExecutionContext::new_root("sess-a");
    assert_ne!(ctx.trace_id, Uuid::nil());
    assert_eq!(ctx.parent_interaction_id, None);
    assert_eq!(ctx.session_id, "sess-a");
}

#[test]
fn two_new_roots_mint_distinct_trace_ids() {
    let a = ExecutionContext::new_root("sess-a");
    let b = ExecutionContext::new_root("sess-b");
    assert_ne!(a.trace_id, b.trace_id);
}

#[test]
fn with_trace_preserves_caller_supplied_trace_id() {
    let trace = Uuid::parse_str("deadbeef-0000-0000-0000-000000000000").unwrap();
    let ctx = ExecutionContext::with_trace(trace, "sess-x");
    assert_eq!(ctx.trace_id, trace);
    assert_eq!(ctx.parent_interaction_id, None);
    assert_eq!(ctx.session_id, "sess-x");
}

#[test]
fn child_inherits_parent_trace_id_unchanged() {
    let parent = ExecutionContext::new_root("sess-a");
    let parent_interaction_id = Uuid::new_v4();
    let child = parent.child(parent_interaction_id, None);
    assert_eq!(child.trace_id, parent.trace_id, "trace inherits");
    assert_eq!(child.parent_interaction_id, Some(parent_interaction_id));
    assert_eq!(child.session_id, parent.session_id);
}

#[test]
fn grandchild_inherits_root_trace_id() {
    let root = ExecutionContext::new_root("sess");
    let mid_interaction = Uuid::new_v4();
    let mid = root.child(mid_interaction, None);
    let grand_interaction = Uuid::new_v4();
    let grand = mid.child(grand_interaction, None);
    assert_eq!(grand.trace_id, root.trace_id, "3-deep trace survives");
    assert_eq!(grand.parent_interaction_id, Some(grand_interaction));
}

#[test]
fn siblings_share_parent_trace_but_have_distinct_parent_interaction_ids() {
    let parent = ExecutionContext::new_root("sess");
    let sib_a_interaction = Uuid::new_v4();
    let sib_b_interaction = Uuid::new_v4();
    let sib_a = parent.child(sib_a_interaction, None);
    let sib_b = parent.child(sib_b_interaction, None);

    assert_eq!(sib_a.trace_id, sib_b.trace_id, "siblings share trace");
    assert_eq!(sib_a.trace_id, parent.trace_id);
    assert_ne!(
        sib_a.parent_interaction_id, sib_b.parent_interaction_id,
        "distinct dispatch ancestors"
    );
}

#[test]
fn child_can_override_session_id_for_spawn_boundary() {
    // When runner:spawn crosses into a new session (e.g. different user
    // or a sandboxed execution), the child gets a new session_id but
    // retains the trace.
    let parent = ExecutionContext::new_root("sess-outer");
    let child = parent.child(Uuid::new_v4(), Some("sess-spawn-42".to_string()));
    assert_eq!(child.trace_id, parent.trace_id);
    assert_eq!(child.session_id, "sess-spawn-42");
    assert_ne!(parent.session_id, child.session_id);
}

#[test]
fn parent_unaffected_by_child_construction() {
    // Rust ownership makes this structurally true, but assert explicitly
    // because the invariant is part of the documented contract.
    let parent = ExecutionContext::new_root("sess");
    let original_trace = parent.trace_id;
    let original_parent_id = parent.parent_interaction_id;
    let _child = parent.child(Uuid::new_v4(), Some("sess-child".to_string()));
    assert_eq!(parent.trace_id, original_trace);
    assert_eq!(parent.parent_interaction_id, original_parent_id);
    assert_eq!(parent.session_id, "sess");
}

#[test]
fn child_fields_serialize_and_deserialize_round_trip() {
    // Sanity: ExecutionContext is serde-backed so it can ride on the
    // wire (e.g. in RunnerIntent for spawn-replay).
    let parent = ExecutionContext::new_root("sess-a");
    let child = parent.child(Uuid::new_v4(), None);
    let json = serde_json::to_value(&child).unwrap();
    let roundtrip: ExecutionContext = serde_json::from_value(json).unwrap();
    assert_eq!(roundtrip.trace_id, child.trace_id);
    assert_eq!(roundtrip.parent_interaction_id, child.parent_interaction_id);
    assert_eq!(roundtrip.session_id, child.session_id);
}
