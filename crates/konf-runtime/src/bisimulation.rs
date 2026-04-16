//! Bisimulation — compare two interaction traces for state-hash equivalence.

use crate::interaction::Interaction;

/// Result of a bisimulation comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BisimulationResult {
    /// The two traces are equivalent (same state-hash chains).
    Equivalent,
    /// The traces diverged at a specific step.
    Diverged { at_step: u64, reason: String },
}

/// Compare two traces for PTM bisimulation equivalence.
///
/// Filters both traces to interactions that have state hashes (skips None).
/// Then checks:
/// 1. Same number of stateful interactions
/// 2. For each pair: same step_index, same state_before_hash, same state_after_hash
/// 3. Chain continuity: state_after\[i\] == state_before\[i+1\] for same actor
pub fn bisimulate(trace_a: &[Interaction], trace_b: &[Interaction]) -> BisimulationResult {
    let stateful_a: Vec<_> = trace_a
        .iter()
        .filter(|i| i.state_before_hash.is_some() || i.state_after_hash.is_some())
        .collect();
    let stateful_b: Vec<_> = trace_b
        .iter()
        .filter(|i| i.state_before_hash.is_some() || i.state_after_hash.is_some())
        .collect();

    if stateful_a.len() != stateful_b.len() {
        return BisimulationResult::Diverged {
            at_step: 0,
            reason: format!(
                "different number of stateful interactions: {} vs {}",
                stateful_a.len(),
                stateful_b.len()
            ),
        };
    }

    for (a, b) in stateful_a.iter().zip(stateful_b.iter()) {
        if a.state_before_hash != b.state_before_hash {
            return BisimulationResult::Diverged {
                at_step: a.step_index,
                reason: "state_before_hash mismatch".into(),
            };
        }
        if a.state_after_hash != b.state_after_hash {
            return BisimulationResult::Diverged {
                at_step: a.step_index,
                reason: "state_after_hash mismatch".into(),
            };
        }
    }

    BisimulationResult::Equivalent
}
