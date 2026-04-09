//! Starter workflow templates — embedded in the binary.
//!
//! These provide sensible defaults that products can customize.
//! Loaded via include_str! so they ship with the binary.
#![allow(dead_code)] // Templates used by project config loader and tests

/// Default chat workflow — simple LLM completion with system prompt.
pub const CHAT_WORKFLOW: &str = include_str!("chat.yaml");

/// Default context assembly — parallel memory search + profile.
pub const CONTEXT_WORKFLOW: &str = include_str!("context.yaml");

/// Default extraction — debounced entity extraction from conversation.
pub const EXTRACTION_WORKFLOW: &str = include_str!("extraction.yaml");

/// Default synthesis — nightly graph maintenance.
pub const SYNTHESIS_WORKFLOW: &str = include_str!("synthesis.yaml");

/// Get a template by name.
pub fn get_template(name: &str) -> Option<&'static str> {
    match name {
        "chat" => Some(CHAT_WORKFLOW),
        "context" => Some(CONTEXT_WORKFLOW),
        "extraction" => Some(EXTRACTION_WORKFLOW),
        "synthesis" => Some(SYNTHESIS_WORKFLOW),
        _ => None,
    }
}

/// List all available template names.
pub fn list_templates() -> Vec<&'static str> {
    vec!["chat", "context", "extraction", "synthesis"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_templates_parse_as_yaml() {
        for name in list_templates() {
            let yaml = get_template(name).unwrap();
            let parsed: serde_yaml::Value = serde_yaml::from_str(yaml)
                .unwrap_or_else(|e| panic!("Template '{name}' is invalid YAML: {e}"));
            assert!(
                parsed.get("workflow").is_some(),
                "Template '{name}' missing 'workflow' key"
            );
            assert!(
                parsed.get("nodes").is_some(),
                "Template '{name}' missing 'nodes' key"
            );
        }
    }

    #[test]
    fn test_get_template_returns_none_for_unknown() {
        assert!(get_template("nonexistent").is_none());
    }
}
