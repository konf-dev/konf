//! Prompt abstraction — parameterized templates that expand into messages.
//!
//! Prompts are user-controlled: users select which prompt to invoke.
//! They expand into a sequence of messages (system, user, assistant roles).
//! Maps to MCP's `prompts/*` methods.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A prompt is a parameterized template that expands into messages.
///
/// Examples: workflow templates from prompts/ directory, system prompts per product mode.
/// Prompts are registered in the engine's PromptRegistry and exposed via MCP.
#[async_trait]
pub trait Prompt: Send + Sync {
    /// Prompt metadata: name, description, arguments.
    fn info(&self) -> PromptInfo;

    /// Expand the prompt with the given arguments into a sequence of messages.
    async fn expand(&self, args: Value) -> Result<Vec<Message>, PromptError>;
}

/// Metadata describing a prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptInfo {
    /// Prompt name, e.g. "code_review", "summarize"
    pub name: String,

    /// Description of what this prompt does
    pub description: String,

    /// Parameters the prompt accepts
    pub arguments: Vec<PromptArgument>,
}

/// A parameter that a prompt accepts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    /// Parameter name
    pub name: String,

    /// Description of what this parameter does
    pub description: String,

    /// Whether this parameter is required
    pub required: bool,
}

/// A message produced by prompt expansion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Role: "user", "assistant", or "system"
    pub role: String,

    /// Message content (string or rich content)
    pub content: Value,
}

/// Errors from prompt operations.
#[derive(Debug, thiserror::Error)]
pub enum PromptError {
    #[error("prompt not found: {0}")]
    NotFound(String),

    #[error("missing required argument: {0}")]
    MissingArgument(String),

    #[error("failed to expand prompt: {0}")]
    ExpansionFailed(String),
}

/// Registry of available prompts, keyed by name.
#[derive(Default, Clone)]
pub struct PromptRegistry {
    prompts: HashMap<String, Arc<dyn Prompt>>,
}

impl PromptRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a prompt. Overwrites any existing prompt with the same name.
    pub fn register(&mut self, prompt: Arc<dyn Prompt>) {
        let name = prompt.info().name.clone();
        self.prompts.insert(name, prompt);
    }

    /// Remove a prompt by name.
    pub fn remove(&mut self, name: &str) -> bool {
        self.prompts.remove(name).is_some()
    }

    /// Get a prompt by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Prompt>> {
        self.prompts.get(name).cloned()
    }

    /// List all registered prompts.
    pub fn list(&self) -> Vec<PromptInfo> {
        self.prompts.values().map(|p| p.info()).collect()
    }

    pub fn len(&self) -> usize {
        self.prompts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.prompts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockPrompt;

    #[async_trait]
    impl Prompt for MockPrompt {
        fn info(&self) -> PromptInfo {
            PromptInfo {
                name: "greet".into(),
                description: "Greet the user".into(),
                arguments: vec![
                    PromptArgument {
                        name: "name".into(),
                        description: "User's name".into(),
                        required: true,
                    },
                ],
            }
        }
        async fn expand(&self, args: Value) -> Result<Vec<Message>, PromptError> {
            let name = args.get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| PromptError::MissingArgument("name".into()))?;
            Ok(vec![
                Message {
                    role: "system".into(),
                    content: Value::String("You are a friendly assistant.".into()),
                },
                Message {
                    role: "user".into(),
                    content: Value::String(format!("Hello, my name is {name}")),
                },
            ])
        }
    }

    #[test]
    fn test_prompt_registry_crud() {
        let mut registry = PromptRegistry::new();
        assert!(registry.is_empty());

        registry.register(Arc::new(MockPrompt));
        assert_eq!(registry.len(), 1);
        assert!(registry.get("greet").is_some());
        assert!(registry.get("nonexistent").is_none());

        let list = registry.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "greet");
        assert_eq!(list[0].arguments.len(), 1);
        assert!(list[0].arguments[0].required);

        assert!(registry.remove("greet"));
        assert!(registry.is_empty());
    }

    #[tokio::test]
    async fn test_prompt_expand() {
        let prompt = MockPrompt;
        let messages = prompt.expand(serde_json::json!({"name": "Alice"})).await.unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[1].content, Value::String("Hello, my name is Alice".into()));
    }

    #[tokio::test]
    async fn test_prompt_expand_missing_arg() {
        let prompt = MockPrompt;
        let result = prompt.expand(serde_json::json!({})).await;
        assert!(result.is_err());
    }
}
