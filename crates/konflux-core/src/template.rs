//! Template rendering using minijinja for {{ }} expressions.

use minijinja::Environment;
use serde_json::Value as JsonValue;
use std::collections::HashMap;

/// Render a template string with the given variables.
pub fn render(template: &str, vars: &HashMap<String, JsonValue>) -> Result<String, String> {
    let mut env = Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    let tmpl = env
        .template_from_str(template)
        .map_err(|e| format!("template parse error: {e}"))?;
    let ctx = minijinja::Value::from_serialize(vars);
    tmpl.render(ctx)
        .map_err(|e| format!("template render error: {e}"))
}

/// Check if a string contains template expressions.
pub fn has_templates(s: &str) -> bool {
    s.contains("{{") && s.contains("}}")
}

/// Resolve all Expr values in a HashMap, rendering templates against state.
pub fn resolve_inputs(
    inputs: &HashMap<String, crate::workflow::Expr>,
    state: &HashMap<String, JsonValue>,
) -> Result<HashMap<String, JsonValue>, String> {
    let mut resolved = HashMap::new();
    for (key, expr) in inputs {
        let value = resolve_expr(expr, state)?;
        resolved.insert(key.clone(), value);
    }
    Ok(resolved)
}

/// Resolve a single Expr against state.
pub fn resolve_expr(
    expr: &crate::workflow::Expr,
    state: &HashMap<String, JsonValue>,
) -> Result<JsonValue, String> {
    match expr {
        crate::workflow::Expr::Literal(s) => Ok(JsonValue::String(s.clone())),
        crate::workflow::Expr::Ref(path) => {
            resolve_ref(path, state).ok_or_else(|| format!("unresolved reference: {path}"))
        }
        crate::workflow::Expr::Template(tmpl) => {
            let rendered = render(tmpl, state)?;
            Ok(JsonValue::String(rendered))
        }
        crate::workflow::Expr::Json(val) => resolve_json_templates(val, state),
    }
}

/// Resolve a dot-path reference against state.
/// "step1.output.text" → state["step1"]["output"]["text"]
fn resolve_ref(path: &str, state: &HashMap<String, JsonValue>) -> Option<JsonValue> {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty() {
        return None;
    }
    let root = state.get(parts[0])?;
    let mut current = root;
    for part in &parts[1..] {
        current = current.get(*part)?;
    }
    Some(current.clone())
}

/// Recursively resolve templates inside JSON values.
fn resolve_json_templates(
    val: &JsonValue,
    state: &HashMap<String, JsonValue>,
) -> Result<JsonValue, String> {
    match val {
        JsonValue::String(s) if has_templates(s) => {
            let rendered = render(s, state)?;
            Ok(JsonValue::String(rendered))
        }
        JsonValue::Array(arr) => {
            let resolved: Result<Vec<_>, _> = arr
                .iter()
                .map(|v| resolve_json_templates(v, state))
                .collect();
            Ok(JsonValue::Array(resolved?))
        }
        JsonValue::Object(obj) => {
            let mut resolved = serde_json::Map::new();
            for (k, v) in obj {
                resolved.insert(k.clone(), resolve_json_templates(v, state)?);
            }
            Ok(JsonValue::Object(resolved))
        }
        other => Ok(other.clone()),
    }
}
