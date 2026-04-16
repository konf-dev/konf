//! B1 — Interaction schema round-trip tests (Phase B, red).
//!
//! Validates that every variant of [`InteractionKind`] and
//! [`InteractionStatus`] serializes to JSON and deserializes back to an
//! equal value, and that unknown fields in input JSON are ignored without
//! error (forward-compatibility for schema evolution).
//!
//! These tests require only the [`Interaction`] type to exist; they do not
//! exercise any runtime behavior. They pass as soon as the type is correctly
//! defined (i.e. immediately after Phase B landing).

use chrono::{TimeZone, Utc};
use konf_runtime::scope::{Actor, ActorRole};
use konf_runtime::{Interaction, InteractionKind, InteractionStatus};
use serde_json::json;
use uuid::Uuid;

/// Test fixture — a prototypical interaction with all fields populated.
fn sample_interaction(kind: InteractionKind, status: InteractionStatus) -> Interaction {
    Interaction {
        id: Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
        parent_id: Some(Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap()),
        trace_id: Uuid::parse_str("00000000-0000-0000-0000-0000000000ff").unwrap(),
        run_id: Some(Uuid::parse_str("00000000-0000-0000-0000-0000000000aa").unwrap()),
        node_id: Some("step_2".to_string()),
        actor: Actor {
            id: "agent:orchestrator".to_string(),
            role: ActorRole::ProductAgent,
        },
        namespace: "konf:orchestrator:user_42".to_string(),
        target: "tool:memory:search".to_string(),
        kind,
        attributes: json!({"example": "value"}),
        edge_rules_fired: vec![
            "cap:memory:*".to_string(),
            "ns_inject:konf:orchestrator:user_42".to_string(),
        ],
        status,
        summary: Some("looked up observations".to_string()),
        timestamp: Utc.with_ymd_and_hms(2026, 4, 14, 9, 33, 12).unwrap(),
    }
}

fn round_trip(original: Interaction) {
    let json = original.to_json();
    let back = Interaction::from_json(json.clone())
        .unwrap_or_else(|e| panic!("deserialization failed: {e}, json was: {json}"));
    // Field-by-field comparison (Interaction does not derive PartialEq —
    // serde_json::Value comparison is sufficient for round-trip equality).
    assert_eq!(back.id, original.id);
    assert_eq!(back.parent_id, original.parent_id);
    assert_eq!(back.trace_id, original.trace_id);
    assert_eq!(back.run_id, original.run_id);
    assert_eq!(back.node_id, original.node_id);
    assert_eq!(back.actor.id, original.actor.id);
    assert_eq!(back.actor.role, original.actor.role);
    assert_eq!(back.namespace, original.namespace);
    assert_eq!(back.target, original.target);
    assert_eq!(back.kind, original.kind);
    assert_eq!(back.attributes, original.attributes);
    assert_eq!(back.edge_rules_fired, original.edge_rules_fired);
    assert_eq!(back.status, original.status);
    assert_eq!(back.summary, original.summary);
    assert_eq!(back.timestamp, original.timestamp);
}

#[test]
fn interaction_tool_dispatch_round_trips() {
    round_trip(sample_interaction(
        InteractionKind::ToolDispatch,
        InteractionStatus::Ok,
    ));
}

#[test]
fn interaction_node_lifecycle_round_trips() {
    round_trip(sample_interaction(
        InteractionKind::NodeLifecycle,
        InteractionStatus::Pending,
    ));
}

#[test]
fn interaction_run_lifecycle_round_trips() {
    round_trip(sample_interaction(
        InteractionKind::RunLifecycle,
        InteractionStatus::Ok,
    ));
}

#[test]
fn interaction_error_round_trips() {
    round_trip(sample_interaction(
        InteractionKind::Error,
        InteractionStatus::Failed {
            error: "tool panicked".to_string(),
        },
    ));
}

#[test]
fn interaction_user_input_round_trips() {
    round_trip(sample_interaction(
        InteractionKind::UserInput,
        InteractionStatus::Observed,
    ));
}

#[test]
fn interaction_llm_response_round_trips() {
    round_trip(sample_interaction(
        InteractionKind::LlmResponse,
        InteractionStatus::Observed,
    ));
}

#[test]
fn interaction_product_defined_round_trips() {
    round_trip(sample_interaction(
        InteractionKind::ProductDefined {
            name: "audit_finding".to_string(),
        },
        InteractionStatus::Observed,
    ));
}

#[test]
fn interaction_status_pending_round_trips() {
    round_trip(sample_interaction(
        InteractionKind::ToolDispatch,
        InteractionStatus::Pending,
    ));
}

#[test]
fn interaction_status_failed_preserves_error_message() {
    let original = sample_interaction(
        InteractionKind::Error,
        InteractionStatus::Failed {
            error: "out of memory: 42".to_string(),
        },
    );
    let json = original.to_json();
    let back = Interaction::from_json(json).unwrap();
    match back.status {
        InteractionStatus::Failed { error } => assert_eq!(error, "out of memory: 42"),
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn interaction_ignores_unknown_fields_for_forward_compat() {
    let mut json =
        sample_interaction(InteractionKind::ToolDispatch, InteractionStatus::Ok).to_json();
    // Inject an unknown field that a future schema version might add.
    let obj = json.as_object_mut().unwrap();
    obj.insert(
        "schema_version".to_string(),
        serde_json::Value::String("v2".to_string()),
    );
    obj.insert(
        "future_only_field".to_string(),
        serde_json::Value::Array(vec![serde_json::Value::from(1)]),
    );

    let back = Interaction::from_json(json)
        .expect("deserialization must tolerate unknown fields for forward-compat");
    assert_eq!(back.kind, InteractionKind::ToolDispatch);
}

#[test]
fn interaction_without_summary_round_trips() {
    let mut original = sample_interaction(InteractionKind::ToolDispatch, InteractionStatus::Ok);
    original.summary = None;
    round_trip(original);
}
