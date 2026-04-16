//! Expression Evaluator Tests

use konflux_substrate::expr::{ExprError, ExprEvaluator, ExprValue};
use std::collections::HashMap;

fn ctx(pairs: &[(&str, ExprValue)]) -> HashMap<String, ExprValue> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[test]
fn test_simple_comparison() {
    let context = ctx(&[("confidence", ExprValue::Float(0.9))]);
    let eval = ExprEvaluator::new(&context);

    assert!(eval.evaluate_as_bool("confidence > 0.8").unwrap());
    assert!(!eval.evaluate_as_bool("confidence > 0.95").unwrap());
    assert!(eval.evaluate_as_bool("confidence >= 0.9").unwrap());
    assert!(eval.evaluate_as_bool("confidence < 1.0").unwrap());
}

#[test]
fn test_string_equality() {
    let context = ctx(&[("category", ExprValue::String("billing".into()))]);
    let eval = ExprEvaluator::new(&context);

    assert!(eval.evaluate_as_bool("category == 'billing'").unwrap());
    assert!(!eval.evaluate_as_bool("category == 'support'").unwrap());
    assert!(eval.evaluate_as_bool("category != 'support'").unwrap());
}

#[test]
fn test_boolean_values() {
    let context = ctx(&[
        ("needs_review", ExprValue::Bool(true)),
        ("is_complete", ExprValue::Bool(false)),
    ]);
    let eval = ExprEvaluator::new(&context);

    assert!(eval.evaluate_as_bool("needs_review == true").unwrap());
    assert!(eval.evaluate_as_bool("is_complete == false").unwrap());
    assert!(!eval.evaluate_as_bool("needs_review == false").unwrap());
}

#[test]
fn test_logical_and() {
    let context = ctx(&[
        ("a", ExprValue::Bool(true)),
        ("b", ExprValue::Bool(true)),
        ("c", ExprValue::Bool(false)),
    ]);
    let eval = ExprEvaluator::new(&context);

    assert!(eval.evaluate_as_bool("a == true && b == true").unwrap());
    assert!(!eval.evaluate_as_bool("a == true && c == true").unwrap());
}

#[test]
fn test_logical_or() {
    let context = ctx(&[("a", ExprValue::Bool(true)), ("b", ExprValue::Bool(false))]);
    let eval = ExprEvaluator::new(&context);

    assert!(eval.evaluate_as_bool("a == true || b == true").unwrap());
    assert!(eval.evaluate_as_bool("b == true || a == true").unwrap());
    assert!(!eval.evaluate_as_bool("b == true || b == true").unwrap());
}

#[test]
fn test_integer_comparison() {
    let context = ctx(&[("count", ExprValue::Int(5))]);
    let eval = ExprEvaluator::new(&context);

    assert!(eval.evaluate_as_bool("count >= 5").unwrap());
    assert!(eval.evaluate_as_bool("count <= 10").unwrap());
    assert!(eval.evaluate_as_bool("count == 5").unwrap());
}

#[test]
fn test_empty_expression() {
    let context = HashMap::new();
    let eval = ExprEvaluator::new(&context);
    assert!(eval.evaluate_as_bool("").unwrap());
    assert!(eval.evaluate_as_bool("   ").unwrap());
}

#[test]
fn test_unknown_reference_error() {
    let context = HashMap::new();
    let eval = ExprEvaluator::new(&context);
    let result = eval.evaluate_as_bool("unknown > 5");
    assert!(matches!(result, Err(ExprError::UnknownReference(_))));
}

#[test]
fn test_nested_reference_with_json() {
    let mut map = serde_json::Map::new();
    map.insert(
        "confidence".to_string(),
        serde_json::Value::Number(serde_json::Number::from_f64(0.85).unwrap()),
    );
    let context = ctx(&[("step1", ExprValue::Json(serde_json::Value::Object(map)))]);
    let eval = ExprEvaluator::new(&context);

    assert!(eval.evaluate_as_bool("step1.confidence > 0.8").unwrap());
}
