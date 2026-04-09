//! Tool guards — configurable deny/allow rules evaluated before tool invocation.
//!
//! Follows the same decorator pattern as [`VirtualizedTool`](crate::context::VirtualizedTool).
//! VirtualizedTool injects parameters; GuardedTool enforces input predicates.
//!
//! # Wrapping order
//!
//! ```text
//! GuardedTool(              ← rules checked on raw LLM input
//!   VirtualizedTool(        ← namespace/bindings injected
//!     inner_tool            ← actual execution
//!   )
//! )
//! ```
//!
//! Rules evaluate BEFORE bindings inject. This means rules operate on what the
//! LLM actually sent, not the post-injection input.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use konflux::error::ToolError;
use konflux::stream::StreamSender;
use konflux::tool::{Tool, ToolContext, ToolInfo};

/// A tool wrapper that evaluates deny/allow rules before delegating to the inner tool.
///
/// Rules are evaluated in order. First match wins:
/// - `Deny` → returns `ToolError::CapabilityDenied`
/// - `Allow` → delegates to inner tool immediately (skips remaining rules)
///
/// If no rule matches, the `default_action` determines behavior.
pub struct GuardedTool {
    inner: Arc<dyn Tool>,
    rules: Vec<Rule>,
    default_action: DefaultAction,
}

impl GuardedTool {
    pub fn new(inner: Arc<dyn Tool>, rules: Vec<Rule>, default_action: DefaultAction) -> Self {
        Self {
            inner,
            rules,
            default_action,
        }
    }

    /// Evaluate rules against the input. Returns `Ok(())` if allowed, `Err` if denied.
    fn evaluate(&self, input: &Value) -> Result<(), ToolError> {
        for rule in &self.rules {
            match rule {
                Rule::Deny { predicate, message } => {
                    if predicate.matches(input) {
                        return Err(ToolError::CapabilityDenied {
                            capability: format!(
                                "guard denied: {}",
                                message
                            ),
                        });
                    }
                }
                Rule::Allow { predicate } => {
                    if predicate.matches(input) {
                        return Ok(());
                    }
                }
            }
        }

        match self.default_action {
            DefaultAction::Allow => Ok(()),
            DefaultAction::Deny => Err(ToolError::CapabilityDenied {
                capability: format!(
                    "guard denied: no rule matched for tool '{}' (default: deny)",
                    self.inner.info().name
                ),
            }),
        }
    }
}

#[async_trait]
impl Tool for GuardedTool {
    fn info(&self) -> ToolInfo {
        self.inner.info()
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        self.evaluate(&input)?;
        self.inner.invoke(input, ctx).await
    }

    async fn invoke_streaming(
        &self,
        input: Value,
        ctx: &ToolContext,
        sender: StreamSender,
    ) -> Result<Value, ToolError> {
        self.evaluate(&input)?;
        self.inner.invoke_streaming(input, ctx, sender).await
    }
}

/// A rule in the guard chain. Evaluated in order; first match wins.
///
/// # YAML format
///
/// ```yaml
/// rules:
///   - action: deny
///     predicate:
///       contains: { path: "command", value: "sudo" }
///     message: "sudo is not allowed"
///   - action: allow
///     predicate:
///       exists: { path: "token" }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Rule {
    Deny {
        predicate: Predicate,
        message: String,
    },
    Allow {
        predicate: Predicate,
    },
}

/// Default behavior when no rule matches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultAction {
    #[default]
    Allow,
    Deny,
}

/// A predicate that tests a JSON input value.
///
/// # YAML format
///
/// ```yaml
/// # Simple predicates
/// predicate:
///   type: contains
///   path: "command"
///   value: "sudo"
///
/// # Composite predicates
/// predicate:
///   type: all
///   predicates:
///     - { type: exists, path: "command" }
///     - { type: contains, path: "command", value: "sudo" }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Predicate {
    /// True if the value at `path` contains `value` as a substring.
    Contains { path: String, value: String },

    /// True if the value at `path` matches a glob pattern.
    Matches { path: String, pattern: String },

    /// True if the value at `path` equals `value` exactly.
    Equals { path: String, value: Value },

    /// True if the field at `path` exists and is not null.
    Exists { path: String },

    /// Negates the inner predicate.
    Not { predicate: Box<Predicate> },

    /// True if all inner predicates are true.
    All { predicates: Vec<Predicate> },

    /// True if any inner predicate is true.
    Any { predicates: Vec<Predicate> },
}

impl Predicate {
    /// Evaluate this predicate against a JSON input.
    pub fn matches(&self, input: &Value) -> bool {
        match self {
            Predicate::Contains { path, value } => {
                resolve_path(input, path)
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s.contains(value.as_str()))
            }
            Predicate::Matches { path, pattern } => {
                resolve_path(input, path)
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| glob_matches(pattern, s))
            }
            Predicate::Equals { path, value } => {
                resolve_path(input, path)
                    .is_some_and(|v| v == value)
            }
            Predicate::Exists { path } => {
                resolve_path(input, path)
                    .is_some_and(|v| !v.is_null())
            }
            Predicate::Not { predicate } => !predicate.matches(input),
            Predicate::All { predicates } => predicates.iter().all(|p| p.matches(input)),
            Predicate::Any { predicates } => predicates.iter().any(|p| p.matches(input)),
        }
    }
}

/// Resolve a dot-separated path against a JSON value.
///
/// Example: `resolve_path({"a": {"b": 1}}, "a.b")` → `Some(1)`
fn resolve_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        match current {
            Value::Object(map) => {
                current = map.get(segment)?;
            }
            Value::Array(arr) => {
                let idx: usize = segment.parse().ok()?;
                current = arr.get(idx)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

/// Simple glob matching supporting `*` (any chars) and `?` (single char).
///
/// This is intentionally minimal — no `**`, no character classes.
/// For the guard use case (matching command strings), this is sufficient.
fn glob_matches(pattern: &str, text: &str) -> bool {
    let p_vec: Vec<char> = pattern.chars().collect();
    let t_vec: Vec<char> = text.chars().collect();
    let mut pi = 0;
    let mut ti = 0;

    // Track backtrack points for `*`
    let mut star_p = None; // position in pattern after the `*`
    let mut star_t = None; // position in text when `*` was matched

    while ti < t_vec.len() {
        if pi < p_vec.len() && (p_vec[pi] == '?' || p_vec[pi] == t_vec[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p_vec.len() && p_vec[pi] == '*' {
            star_p = Some(pi + 1);
            star_t = Some(ti);
            pi += 1;
        } else if let (Some(sp), Some(st)) = (star_p, star_t) {
            pi = sp;
            ti = st + 1;
            star_t = Some(ti);
        } else {
            return false;
        }
    }

    // Consume remaining `*` in pattern
    while pi < p_vec.len() && p_vec[pi] == '*' {
        pi += 1;
    }

    pi == p_vec.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    // -- Predicate tests --

    #[test]
    fn contains_matches_substring() {
        let pred = Predicate::Contains {
            path: "command".into(),
            value: "sudo".into(),
        };
        assert!(pred.matches(&json!({"command": "sudo rm -rf /"})));
        assert!(!pred.matches(&json!({"command": "ls /tmp"})));
    }

    #[test]
    fn contains_returns_false_for_missing_path() {
        let pred = Predicate::Contains {
            path: "command".into(),
            value: "sudo".into(),
        };
        assert!(!pred.matches(&json!({"other": "sudo"})));
    }

    #[test]
    fn contains_returns_false_for_non_string() {
        let pred = Predicate::Contains {
            path: "command".into(),
            value: "sudo".into(),
        };
        assert!(!pred.matches(&json!({"command": 42})));
    }

    #[test]
    fn matches_glob_star() {
        let pred = Predicate::Matches {
            path: "command".into(),
            pattern: "rm -rf*".into(),
        };
        assert!(pred.matches(&json!({"command": "rm -rf /"})));
        assert!(pred.matches(&json!({"command": "rm -rf /home"})));
        assert!(!pred.matches(&json!({"command": "ls /tmp"})));
    }

    #[test]
    fn matches_glob_question_mark() {
        let pred = Predicate::Matches {
            path: "command".into(),
            pattern: "cat ?.txt".into(),
        };
        assert!(pred.matches(&json!({"command": "cat a.txt"})));
        assert!(!pred.matches(&json!({"command": "cat ab.txt"})));
    }

    #[test]
    fn equals_exact_match() {
        let pred = Predicate::Equals {
            path: "mode".into(),
            value: json!("destructive"),
        };
        assert!(pred.matches(&json!({"mode": "destructive"})));
        assert!(!pred.matches(&json!({"mode": "safe"})));
    }

    #[test]
    fn equals_numeric() {
        let pred = Predicate::Equals {
            path: "retries".into(),
            value: json!(0),
        };
        assert!(pred.matches(&json!({"retries": 0})));
        assert!(!pred.matches(&json!({"retries": 1})));
    }

    #[test]
    fn exists_present_field() {
        let pred = Predicate::Exists { path: "token".into() };
        assert!(pred.matches(&json!({"token": "abc"})));
        assert!(!pred.matches(&json!({"token": null})));
        assert!(!pred.matches(&json!({"other": "abc"})));
    }

    #[test]
    fn not_negates() {
        let pred = Predicate::Not { predicate: Box::new(Predicate::Exists { path: "admin".into() }) };
        assert!(pred.matches(&json!({"user": "alice"})));
        assert!(!pred.matches(&json!({"admin": true})));
    }

    #[test]
    fn all_requires_all_true() {
        let pred = Predicate::All { predicates: vec![
            Predicate::Exists { path: "command".into() },
            Predicate::Contains { path: "command".into(), value: "sudo".into() },
        ] };
        assert!(pred.matches(&json!({"command": "sudo ls"})));
        assert!(!pred.matches(&json!({"command": "ls"})));
    }

    #[test]
    fn any_requires_one_true() {
        let pred = Predicate::Any { predicates: vec![
            Predicate::Contains { path: "command".into(), value: "sudo".into() },
            Predicate::Contains { path: "command".into(), value: "rm".into() },
        ] };
        assert!(pred.matches(&json!({"command": "sudo ls"})));
        assert!(pred.matches(&json!({"command": "rm file"})));
        assert!(!pred.matches(&json!({"command": "ls"})));
    }

    #[test]
    fn nested_path_resolution() {
        let pred = Predicate::Equals {
            path: "config.level".into(),
            value: json!("admin"),
        };
        assert!(pred.matches(&json!({"config": {"level": "admin"}})));
        assert!(!pred.matches(&json!({"config": {"level": "user"}})));
    }

    #[test]
    fn array_index_resolution() {
        let pred = Predicate::Equals {
            path: "items.0.name".into(),
            value: json!("first"),
        };
        assert!(pred.matches(&json!({"items": [{"name": "first"}]})));
        assert!(!pred.matches(&json!({"items": [{"name": "second"}]})));
    }

    // -- glob_matches tests --

    #[test]
    fn glob_exact() {
        assert!(glob_matches("hello", "hello"));
        assert!(!glob_matches("hello", "world"));
    }

    #[test]
    fn glob_star_suffix() {
        assert!(glob_matches("rm*", "rm -rf /"));
        assert!(glob_matches("rm*", "rm"));
        assert!(!glob_matches("rm*", "ls"));
    }

    #[test]
    fn glob_star_prefix() {
        assert!(glob_matches("*.txt", "file.txt"));
        assert!(!glob_matches("*.txt", "file.rs"));
    }

    #[test]
    fn glob_star_middle() {
        assert!(glob_matches("a*z", "az"));
        assert!(glob_matches("a*z", "abcz"));
        assert!(!glob_matches("a*z", "abcy"));
    }

    #[test]
    fn glob_question() {
        assert!(glob_matches("a?c", "abc"));
        assert!(!glob_matches("a?c", "abbc"));
    }

    #[test]
    fn glob_empty() {
        assert!(glob_matches("", ""));
        assert!(!glob_matches("", "a"));
        assert!(glob_matches("*", ""));
        assert!(glob_matches("*", "anything"));
    }

    // -- GuardedTool tests --

    struct MockTool;

    #[async_trait]
    impl Tool for MockTool {
        fn info(&self) -> ToolInfo {
            ToolInfo {
                name: "mock".into(),
                description: "test mock".into(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                capabilities: vec![],
                supports_streaming: false,
                annotations: Default::default(),
            }
        }
        async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
            Ok(input)
        }
    }

    fn test_ctx() -> ToolContext {
        ToolContext {
            capabilities: vec![],
            workflow_id: "test".into(),
            node_id: "test".into(),
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn deny_rule_blocks_matching_input() {
        let tool = GuardedTool::new(
            Arc::new(MockTool),
            vec![Rule::Deny {
                predicate: Predicate::Contains {
                    path: "command".into(),
                    value: "sudo".into(),
                },
                message: "sudo is not allowed".into(),
            }],
            DefaultAction::Allow,
        );

        let result = tool.invoke(json!({"command": "sudo rm -rf /"}), &test_ctx()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("sudo is not allowed"), "got: {err}");
    }

    #[tokio::test]
    async fn deny_rule_passes_non_matching_input() {
        let tool = GuardedTool::new(
            Arc::new(MockTool),
            vec![Rule::Deny {
                predicate: Predicate::Contains {
                    path: "command".into(),
                    value: "sudo".into(),
                },
                message: "sudo is not allowed".into(),
            }],
            DefaultAction::Allow,
        );

        let result = tool.invoke(json!({"command": "ls /tmp"}), &test_ctx()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn allow_rule_short_circuits() {
        let tool = GuardedTool::new(
            Arc::new(MockTool),
            vec![
                Rule::Allow {
                    predicate: Predicate::Equals {
                        path: "command".into(),
                        value: json!("ls"),
                    },
                },
                Rule::Deny {
                    predicate: Predicate::Exists { path: "command".into() },
                    message: "all commands denied".into(),
                },
            ],
            DefaultAction::Deny,
        );

        // "ls" matches the Allow rule first → passes
        let result = tool.invoke(json!({"command": "ls"}), &test_ctx()).await;
        assert!(result.is_ok());

        // "rm" doesn't match Allow, hits Deny → blocked
        let result = tool.invoke(json!({"command": "rm"}), &test_ctx()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn default_deny_blocks_unmatched() {
        let tool = GuardedTool::new(
            Arc::new(MockTool),
            vec![Rule::Allow {
                predicate: Predicate::Equals {
                    path: "command".into(),
                    value: json!("ls"),
                },
            }],
            DefaultAction::Deny,
        );

        let result = tool.invoke(json!({"command": "cat /etc/passwd"}), &test_ctx()).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("default: deny"), "got: {err}");
    }

    #[tokio::test]
    async fn default_allow_passes_unmatched() {
        let tool = GuardedTool::new(
            Arc::new(MockTool),
            vec![Rule::Deny {
                predicate: Predicate::Contains {
                    path: "command".into(),
                    value: "sudo".into(),
                },
                message: "no sudo".into(),
            }],
            DefaultAction::Allow,
        );

        let result = tool.invoke(json!({"command": "ls"}), &test_ctx()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn no_rules_with_default_allow() {
        let tool = GuardedTool::new(
            Arc::new(MockTool),
            vec![],
            DefaultAction::Allow,
        );
        let result = tool.invoke(json!({"anything": true}), &test_ctx()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn no_rules_with_default_deny() {
        let tool = GuardedTool::new(
            Arc::new(MockTool),
            vec![],
            DefaultAction::Deny,
        );
        let result = tool.invoke(json!({"anything": true}), &test_ctx()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn info_delegates_to_inner() {
        let tool = GuardedTool::new(
            Arc::new(MockTool),
            vec![],
            DefaultAction::Allow,
        );
        assert_eq!(tool.info().name, "mock");
    }

    // -- resolve_path tests --

    #[test]
    fn resolve_top_level() {
        let val = json!({"key": "value"});
        assert_eq!(resolve_path(&val, "key"), Some(&json!("value")));
    }

    #[test]
    fn resolve_nested() {
        let val = json!({"a": {"b": {"c": 42}}});
        assert_eq!(resolve_path(&val, "a.b.c"), Some(&json!(42)));
    }

    #[test]
    fn resolve_missing() {
        let val = json!({"a": 1});
        assert_eq!(resolve_path(&val, "b"), None);
    }

    #[test]
    fn resolve_array() {
        let val = json!({"items": [10, 20, 30]});
        assert_eq!(resolve_path(&val, "items.1"), Some(&json!(20)));
    }
}
