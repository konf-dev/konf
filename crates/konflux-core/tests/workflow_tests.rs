//! Workflow IR Tests

use konflux::workflow::{Workflow, Step, EdgeTarget};
use konflux::error::ValidationError;

#[test]
fn test_valid_linear_workflow() {
    // A → B → C → RETURN
    let workflow = Workflow::new("w1", "Linear", "a")
        .with_step(
            Step::new("a", "tool1")
                .with_edge(EdgeTarget::Step("b".into()))
        )
        .with_step(
            Step::new("b", "tool2")
                .with_edge(EdgeTarget::Step("c".into()))
        )
        .with_step(
            Step::new("c", "tool3")
                .with_edge(EdgeTarget::Return)
        );

    assert!(workflow.validate().is_ok());
}

#[test]
fn test_valid_diamond_workflow() {
    //     → B →
    //   /       \
    // A           D → RETURN
    //   \       /
    //     → C →
    let workflow = Workflow::new("w2", "Diamond", "a")
        .with_step(
            Step::new("a", "tool1")
                .with_edge(EdgeTarget::Step("b".into()))
                .with_edge(EdgeTarget::Step("c".into()))
        )
        .with_step(
            Step::new("b", "tool2")
                .with_edge(EdgeTarget::Step("d".into()))
        )
        .with_step(
            Step::new("c", "tool3")
                .with_edge(EdgeTarget::Step("d".into()))
        )
        .with_step(
            Step::new("d", "tool4")
                .with_edge(EdgeTarget::Return)
        );

    // D depends on B and C implicitly via edges, but we can also add explicit depends_on
    let d = Step::new("d", "tool4")
        .with_edge(EdgeTarget::Return)
        .with_depends_on("b")
        .with_depends_on("c");
    
    let workflow = workflow.with_step(d);

    assert!(workflow.validate().is_ok());
}

#[test]
fn test_cycle_detection_simple() {
    // A → B → A (cycle!)
    let workflow = Workflow::new("w3", "Cycle", "a")
        .with_step(
            Step::new("a", "tool1")
                .with_edge(EdgeTarget::Step("b".into()))
        )
        .with_step(
            Step::new("b", "tool2")
                .with_edge(EdgeTarget::Step("a".into()))
        )
        .with_step(Step::new("c", "tool3").with_edge(EdgeTarget::Return)); // Need a return for validation

    let result = workflow.validate();
    assert!(result.is_err());
    
    if let Err(ValidationError::CycleDetected { path }) = result {
        assert!(path.len() >= 2);
    } else {
        panic!("Expected CycleDetected error, got {:?}", result);
    }
}

#[test]
fn test_missing_entry_error() {
    let workflow = Workflow::new("w6", "NoEntry", "nonexistent")
        .with_step(Step::new("a", "tool1").with_edge(EdgeTarget::Return));

    let result = workflow.validate();
    assert!(matches!(result, Err(ValidationError::NoEntryNode)));
}

#[test]
fn test_missing_return_error() {
    let workflow = Workflow::new("w8", "NoReturn", "a")
        .with_step(Step::new("a", "tool1").with_edge(EdgeTarget::Step("b".into())))
        .with_step(Step::new("b", "tool2"));

    let result = workflow.validate();
    assert!(matches!(result, Err(ValidationError::NoReturnNode)));
}
