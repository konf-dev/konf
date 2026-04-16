//! Standard library tools for managing secrets via environment variables.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use konflux_substrate::envelope::Envelope;
use konflux_substrate::error::ToolError;
use konflux_substrate::tool::{Tool, ToolAnnotations, ToolInfo};
use konflux_substrate::Engine;

/// Configuration for the secret tool.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SecretConfig {
    /// List of environment variable names that are allowed to be read.
    pub allowed_keys: Vec<String>,
}

/// Register secret tools with the engine.
pub fn register(engine: &Engine, config: &SecretConfig) {
    engine.register_tool(Arc::new(SecretGetTool {
        allowed_keys: config.allowed_keys.clone(),
    }));
    engine.register_tool(Arc::new(SecretListTool {
        allowed_keys: config.allowed_keys.clone(),
    }));
}

// ============================================================
// secret:get
// ============================================================

pub struct SecretGetTool {
    allowed_keys: Vec<String>,
}

#[async_trait]
impl Tool for SecretGetTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "secret:get".into(),
            description: "Fetch a secret from the environment. Only keys listed in 'allowed_keys' are accessible.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string" }
                },
                "required": ["key"]
            }),
            output_schema: Some(json!({ "type": "string" })),
            capabilities: vec!["secret:get".into()],
            supports_streaming: false,
            annotations: ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            },
        }
    }

    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        let key = env
            .payload
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing 'key'".into(),
                field: Some("key".into()),
            })?;

        if !self.allowed_keys.contains(&key.to_string()) {
            return Err(ToolError::AccessDenied {
                message: format!("Secret key '{}' is not in the allowed_keys whitelist.", key),
            });
        }

        match std::env::var(key) {
            Ok(val) => Ok(env.respond(Value::String(val))),
            Err(_) => Err(ToolError::ExecutionFailed {
                message: format!(
                    "Secret key '{}' is allowed but not found in the environment.",
                    key
                ),
                retryable: false,
            }),
        }
    }
}

// ============================================================
// secret:list
// ============================================================

pub struct SecretListTool {
    allowed_keys: Vec<String>,
}

#[async_trait]
impl Tool for SecretListTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "secret:list".into(),
            description: "List the names of all allowed secrets. Does NOT return their values."
                .into(),
            input_schema: json!({ "type": "object" }),
            output_schema: Some(json!({
                "type": "array",
                "items": { "type": "string" }
            })),
            capabilities: vec!["secret:list".into()],
            supports_streaming: false,
            annotations: ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            },
        }
    }

    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        Ok(env.respond(json!(self.allowed_keys)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_secret_get_allowed() {
        std::env::set_var("TEST_SECRET_ALLOWED", "secret_value");
        let tool = SecretGetTool {
            allowed_keys: vec!["TEST_SECRET_ALLOWED".into()],
        };
        let result = tool
            .invoke(Envelope::test(json!({ "key": "TEST_SECRET_ALLOWED" })))
            .await
            .unwrap();
        assert_eq!(result.payload, Value::String("secret_value".into()));
    }

    #[tokio::test]
    async fn test_secret_get_denied() {
        std::env::set_var("TEST_SECRET_HIDDEN", "hidden_value");
        let tool = SecretGetTool {
            allowed_keys: vec!["OTHER_KEY".into()],
        };
        let result = tool
            .invoke(Envelope::test(json!({ "key": "TEST_SECRET_HIDDEN" })))
            .await;
        match result {
            Err(ToolError::AccessDenied { .. }) => (),
            _ => panic!("Expected AccessDenied error"),
        }
    }

    #[tokio::test]
    async fn test_secret_get_missing() {
        let tool = SecretGetTool {
            allowed_keys: vec!["MISSING_KEY".into()],
        };
        let result = tool
            .invoke(Envelope::test(json!({ "key": "MISSING_KEY" })))
            .await;
        match result {
            Err(ToolError::ExecutionFailed { .. }) => (),
            _ => panic!("Expected ExecutionFailed error"),
        }
    }

    #[tokio::test]
    async fn test_secret_list() {
        let tool = SecretListTool {
            allowed_keys: vec!["A".into(), "B".into()],
        };
        let result = tool.invoke(Envelope::test(json!({}))).await.unwrap();
        assert_eq!(result.payload, json!(["A", "B"]));
    }
}
