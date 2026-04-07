use std::collections::HashMap;
use serde_json::json;
use konflux::template::{render, has_templates, resolve_expr};
use konflux::workflow::Expr;

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
    assert_eq!(result, json!({
        "greeting": "Hello Alice",
        "meta": {
            "id": 123
        }
    }));
}

#[test]
fn test_missing_variable_error() {
    let vars = HashMap::new();
    let result = render("Hello {{ name }}!", &vars);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("template render error"));
}
