//! Tests for PTM bisimulation harness and state projection.

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use konf_runtime::bisimulation::{bisimulate, BisimulationResult};
use konf_runtime::interaction::{Interaction, InteractionKind, InteractionStatus};
use konf_runtime::scope::{Actor, ActorRole};
use konflux_substrate::projection::Projection;

fn make_interaction(
    step_index: u64,
    state_before: Option<[u8; 32]>,
    state_after: Option<[u8; 32]>,
) -> Interaction {
    Interaction {
        id: Uuid::new_v4(),
        parent_id: None,
        trace_id: Uuid::new_v4(),
        run_id: None,
        node_id: None,
        actor: Actor {
            id: "test".into(),
            role: ActorRole::User,
        },
        namespace: "test".into(),
        target: "tool:echo".into(),
        kind: InteractionKind::ToolDispatch,
        attributes: json!({}),
        edge_rules_fired: vec![],
        status: InteractionStatus::Ok,
        summary: None,
        timestamp: Utc::now(),
        step_index,
        stream_id: String::new(),
        state_before_hash: state_before,
        state_after_hash: state_after,
        references: vec![],
        in_reply_to: None,
    }
}

#[test]
fn bisimulation_smoke_equivalent() {
    let hash_a = Projection::new(b"state_a".to_vec()).hash();
    let hash_b = Projection::new(b"state_b".to_vec()).hash();

    let trace = vec![
        make_interaction(0, Some(hash_a), Some(hash_b)),
        make_interaction(1, Some(hash_b), Some(hash_a)),
    ];

    // Identical traces should be equivalent.
    let result = bisimulate(&trace, &trace);
    assert_eq!(result, BisimulationResult::Equivalent);
}

#[test]
fn bisimulation_diverged() {
    let hash_a = Projection::new(b"state_a".to_vec()).hash();
    let hash_b = Projection::new(b"state_b".to_vec()).hash();
    let hash_c = Projection::new(b"state_c".to_vec()).hash();

    let trace_a = vec![make_interaction(0, Some(hash_a), Some(hash_b))];
    let trace_b = vec![make_interaction(0, Some(hash_a), Some(hash_c))];

    let result = bisimulate(&trace_a, &trace_b);
    match result {
        BisimulationResult::Diverged { at_step, reason } => {
            assert_eq!(at_step, 0);
            assert!(reason.contains("state_after_hash"));
        }
        other => panic!("expected Diverged, got {other:?}"),
    }
}

#[test]
fn state_projection_deterministic() {
    let bytes = b"deterministic_state_data";
    let proj_1 = Projection::new(bytes.to_vec());
    let proj_2 = Projection::new(bytes.to_vec());
    assert_eq!(proj_1.hash(), proj_2.hash());
}
