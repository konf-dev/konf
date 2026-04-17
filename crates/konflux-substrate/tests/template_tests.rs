use konflux_substrate::template::{has_templates, render, resolve_expr};
use konflux_substrate::workflow::Expr;
use serde_json::json;
use std::collections::HashMap;

#[test]
fn test_simple_render() {
    let mut vars = HashMap::new();
    vars.insert("name".to_string(), json!("World"));
    let result = render("Hello {{ name }}!", &vars).unwrap();
    assert_eq!(result, "Hello World!");
}

#[test]
fn test_nested_access() {
    let mut vars = HashMap::new();
    vars.insert("user".to_string(), json!({"profile": {"name": "Alice"}}));
    let result = render("Hello {{ user.profile.name }}!", &vars).unwrap();
    assert_eq!(result, "Hello Alice!");
}

#[test]
fn test_has_templates() {
    assert!(has_templates("Hello {{ name }}!"));
    assert!(!has_templates("Hello World!"));
}

#[test]
fn test_resolve_expr_literal() {
    let vars = HashMap::new();
    let expr = Expr::Literal("hello".to_string());
    let result = resolve_expr(&expr, &vars).unwrap();
    assert_eq!(result, json!("hello"));
}

#[test]
fn test_resolve_expr_ref() {
    let mut vars = HashMap::new();
    vars.insert("foo".to_string(), json!({"bar": "baz"}));
    let expr = Expr::Ref("foo.bar".to_string());
    let result = resolve_expr(&expr, &vars).unwrap();
    assert_eq!(result, json!("baz"));
}

#[test]
fn test_resolve_expr_template() {
    let mut vars = HashMap::new();
    vars.insert("name".to_string(), json!("Alice"));
    let expr = Expr::Template("Hello {{ name }}".to_string());
    let result = resolve_expr(&expr, &vars).unwrap();
    assert_eq!(result, json!("Hello Alice"));
}

#[test]
fn test_resolve_expr_json_with_templates() {
    let mut vars = HashMap::new();
    vars.insert("name".to_string(), json!("Alice"));
    let expr = Expr::Json(json!({
        "greeting": "Hello {{ name }}",
        "meta": {
            "id": 123
        }
    }));
    let result = resolve_expr(&expr, &vars).unwrap();
    assert_eq!(
        result,
        json!({
            "greeting": "Hello Alice",
            "meta": {
                "id": 123
            }
        })
    );
}

#[test]
fn test_missing_variable_error() {
    let vars = HashMap::new();
    let result = render("Hello {{ name }}!", &vars);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("template render error"));
}

// ------------------------------------------------------------------
// Pure-reference passthrough — `"{{ foo }}"` preserves foo's JSON type.
// See template.rs module docs for motivation (multi-turn chat history).
// ------------------------------------------------------------------

#[test]
fn test_pure_ref_passthrough_preserves_array() {
    let mut vars = HashMap::new();
    vars.insert(
        "history".to_string(),
        json!([{"role": "user", "content": "hi"}]),
    );
    // A `with:` block containing a single `{{ history }}` should produce
    // the array itself, not its stringified form.
    let expr = Expr::Json(json!({ "messages": "{{history}}" }));
    let result = resolve_expr(&expr, &vars).unwrap();
    assert_eq!(
        result,
        json!({ "messages": [{"role": "user", "content": "hi"}] })
    );
}

#[test]
fn test_pure_ref_passthrough_preserves_object() {
    let mut vars = HashMap::new();
    vars.insert("meta".to_string(), json!({"model": "qwen3", "tokens": 42}));
    let expr = Expr::Json(json!({ "out": "{{meta}}" }));
    let result = resolve_expr(&expr, &vars).unwrap();
    assert_eq!(result, json!({ "out": {"model": "qwen3", "tokens": 42} }));
}

#[test]
fn test_pure_ref_passthrough_preserves_number() {
    let mut vars = HashMap::new();
    vars.insert("count".to_string(), json!(7));
    let expr = Expr::Json(json!({ "n": "{{count}}" }));
    let result = resolve_expr(&expr, &vars).unwrap();
    assert_eq!(result, json!({ "n": 7 }));
}

#[test]
fn test_pure_ref_passthrough_preserves_bool() {
    let mut vars = HashMap::new();
    vars.insert("ok".to_string(), json!(true));
    let expr = Expr::Json(json!({ "flag": "{{ok}}" }));
    let result = resolve_expr(&expr, &vars).unwrap();
    assert_eq!(result, json!({ "flag": true }));
}

#[test]
fn test_pure_ref_nested_path_preserves_type() {
    let mut vars = HashMap::new();
    vars.insert("run".to_string(), json!({"output": {"results": [1, 2, 3]}}));
    let expr = Expr::Json(json!({ "xs": "{{run.output.results}}" }));
    let result = resolve_expr(&expr, &vars).unwrap();
    assert_eq!(result, json!({ "xs": [1, 2, 3] }));
}

#[test]
fn test_pure_ref_whitespace_tolerant() {
    let mut vars = HashMap::new();
    vars.insert("arr".to_string(), json!([1, 2]));
    for form in ["{{arr}}", "{{ arr }}", "{{   arr   }}", " {{arr}} "] {
        let expr = Expr::Json(json!({ "x": form }));
        let result = resolve_expr(&expr, &vars).unwrap();
        assert_eq!(result, json!({ "x": [1, 2] }), "form: {form:?}");
    }
}

#[test]
fn test_mixed_text_still_renders_as_string() {
    let mut vars = HashMap::new();
    vars.insert("name".to_string(), json!("Alice"));
    // Surrounding text means it's NOT a pure ref — falls through to minijinja.
    let expr = Expr::Json(json!({ "greet": "Hello {{name}}!" }));
    let result = resolve_expr(&expr, &vars).unwrap();
    assert_eq!(result, json!({ "greet": "Hello Alice!" }));
}

#[test]
fn test_multiple_exprs_still_render_as_string() {
    let mut vars = HashMap::new();
    vars.insert("a".to_string(), json!("x"));
    vars.insert("b".to_string(), json!("y"));
    let expr = Expr::Json(json!({ "c": "{{a}}{{b}}" }));
    let result = resolve_expr(&expr, &vars).unwrap();
    assert_eq!(result, json!({ "c": "xy" }));
}

#[test]
fn test_filter_still_renders_as_string() {
    let mut vars = HashMap::new();
    vars.insert("name".to_string(), json!("alice"));
    // `| upper` contains `|`, so pure-ref detection rejects — fine.
    let expr = Expr::Json(json!({ "n": "{{name | upper}}" }));
    let result = resolve_expr(&expr, &vars).unwrap();
    assert_eq!(result, json!({ "n": "ALICE" }));
}

#[test]
fn test_pure_ref_missing_path_falls_through_to_strict_error() {
    let vars = HashMap::new();
    let expr = Expr::Json(json!({ "x": "{{nope}}" }));
    let result = resolve_expr(&expr, &vars);
    // resolve_ref returns None → we drop into render(), minijinja strict
    // mode raises. Caller sees a descriptive error, not a silent empty.
    assert!(result.is_err(), "expected error, got: {result:?}");
}

#[test]
fn test_raw_block_still_renders_unchanged() {
    let mut vars = HashMap::new();
    vars.insert("name".to_string(), json!("Alice"));
    // {% raw %} contains `{%` which isn't a `{{`-expression opener, but
    // `try_pure_ref` rejects it anyway because the whole string isn't
    // just `{{ref}}`. Verifies the existing {% raw %} workaround still
    // passes through to minijinja which emits the literal body.
    let expr = Expr::Json(json!({
        "template": "{% raw %}Hello {{name}}{% endraw %}"
    }));
    let result = resolve_expr(&expr, &vars).unwrap();
    assert_eq!(result, json!({ "template": "Hello {{name}}" }));
}
