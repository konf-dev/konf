//! Template rendering using minijinja for {{ }} expressions.
//!
//! ## Pure-reference passthrough
//!
//! When a string value in a workflow's `with:` block is ONLY a single
//! reference expression — e.g. `"{{ prior.messages }}"` with no
//! surrounding text, filters, or additional expressions — we bypass
//! minijinja and return the referenced value directly, preserving its
//! original JSON type (array, object, number, bool, string, null).
//!
//! Without this, minijinja stringifies the value as its Python-like
//! repr, so arrays and objects can't be passed from one node's output
//! to another node's structured input. That blocked multi-turn chat
//! patterns like `ai:complete with messages: "{{history.value}}"`.
//!
//! Strings that are NOT pure refs (have surrounding text, filters,
//! multiple expressions, or block tags like `{% raw %}`) continue
//! through minijinja exactly as before — no backward-compat break.

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

/// If `s` is exactly a single reference expression `{{ path.to.ref }}`
/// (with optional whitespace, no surrounding text, no filters, no
/// pipe operators, no block tags), return the path. Otherwise None.
///
/// "path" is one or more dot-separated identifier segments. Identifiers
/// are the usual alphanumeric + underscore set used by konflux's
/// `resolve_ref`. Anything outside that — `[0]` array indexing, `|`
/// filters, function calls, etc. — falls through to minijinja.
fn try_pure_ref(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    let inner = trimmed.strip_prefix("{{")?.strip_suffix("}}")?.trim();

    // No nested `{{` or `}}` — that would mean multiple expressions.
    if inner.contains("{{") || inner.contains("}}") {
        return None;
    }
    // No block tags, filters, comparisons, function calls, arithmetic.
    if inner.chars().any(|c| {
        matches!(
            c,
            '|' | '(' | ')' | '[' | ']' | '+' | '-' | '=' | '<' | '>' | ','
        )
    }) {
        return None;
    }
    // Path must be non-empty and composed of identifier characters
    // and dots. Every dot must separate non-empty identifier segments.
    if inner.is_empty() {
        return None;
    }
    let valid = inner.split('.').all(|seg| {
        !seg.is_empty()
            && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !seg.chars().next().is_some_and(|c| c.is_ascii_digit())
    });
    if !valid {
        return None;
    }
    Some(inner)
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
            // Pure-ref passthrough: if the string is exactly a single
            // `{{ path.to.ref }}` expression and the path resolves,
            // return the referenced JsonValue with its original type.
            // This unblocks passing arrays/objects between nodes.
            if let Some(path) = try_pure_ref(s) {
                if let Some(value) = resolve_ref(path, state) {
                    return Ok(value);
                }
                // Path didn't resolve — fall through so minijinja's
                // strict-undefined raises a clear error.
            }
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
