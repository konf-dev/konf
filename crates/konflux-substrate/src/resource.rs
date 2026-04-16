//! Resource abstraction — read-only context exposed to agents and MCP clients.
//!
//! Resources are app-controlled: the application decides what to expose.
//! Agents can browse and read resources but not modify them.
//! Maps to MCP's `resources/*` methods.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

/// A resource is read-only context that the application exposes.
///
/// Examples: product config files, workflow definitions, memory schema, audit journal.
/// Resources are registered in the engine's ResourceRegistry and exposed via MCP.
#[async_trait]
pub trait Resource: Send + Sync {
    /// Resource metadata: URI, name, description, MIME type.
    fn info(&self) -> ResourceInfo;

    /// Read the resource's current content.
    async fn read(&self) -> Result<Value, ResourceError>;

    /// Subscribe to change notifications. Returns None if the resource
    /// does not support subscriptions (the default).
    fn subscribe(&self) -> Option<broadcast::Receiver<ResourceChanged>> {
        None
    }
}

/// Notification that a resource has changed.
#[derive(Debug, Clone)]
pub struct ResourceChanged {
    /// URI of the changed resource
    pub uri: String,
}

/// Metadata describing a resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceInfo {
    /// URI identifying this resource, e.g. "konf://config/tools.yaml"
    pub uri: String,

    /// Human-readable name
    pub name: String,

    /// Description of what this resource contains
    pub description: String,

    /// MIME type (e.g. "application/yaml", "application/json", "text/plain")
    pub mime_type: String,
}

/// Errors from resource operations.
#[derive(Debug, thiserror::Error)]
pub enum ResourceError {
    #[error("resource not found: {0}")]
    NotFound(String),

    #[error("failed to read resource: {0}")]
    ReadFailed(String),
}

/// Registry of available resources, keyed by URI.
#[derive(Default, Clone)]
pub struct ResourceRegistry {
    resources: HashMap<String, Arc<dyn Resource>>,
}

impl ResourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a resource. Overwrites any existing resource with the same URI.
    pub fn register(&mut self, resource: Arc<dyn Resource>) {
        let uri = resource.info().uri.clone();
        self.resources.insert(uri, resource);
    }

    /// Remove a resource by URI.
    pub fn remove(&mut self, uri: &str) -> bool {
        self.resources.remove(uri).is_some()
    }

    /// Get a resource by URI.
    pub fn get(&self, uri: &str) -> Option<Arc<dyn Resource>> {
        self.resources.get(uri).cloned()
    }

    /// List all registered resources.
    pub fn list(&self) -> Vec<ResourceInfo> {
        self.resources.values().map(|r| r.info()).collect()
    }

    pub fn len(&self) -> usize {
        self.resources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockResource {
        uri: String,
        content: Value,
    }

    #[async_trait]
    impl Resource for MockResource {
        fn info(&self) -> ResourceInfo {
            ResourceInfo {
                uri: self.uri.clone(),
                name: "test".into(),
                description: "test resource".into(),
                mime_type: "application/json".into(),
            }
        }
        async fn read(&self) -> Result<Value, ResourceError> {
            Ok(self.content.clone())
        }
    }

    #[test]
    fn test_resource_registry_crud() {
        let mut registry = ResourceRegistry::new();
        assert!(registry.is_empty());

        registry.register(Arc::new(MockResource {
            uri: "konf://test/a".into(),
            content: serde_json::json!({"key": "value"}),
        }));
        assert_eq!(registry.len(), 1);
        assert!(registry.get("konf://test/a").is_some());
        assert!(registry.get("konf://test/b").is_none());

        let list = registry.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].uri, "konf://test/a");

        assert!(registry.remove("konf://test/a"));
        assert!(registry.is_empty());
        assert!(!registry.remove("nonexistent"));
    }

    #[tokio::test]
    async fn test_resource_read() {
        let resource = MockResource {
            uri: "konf://test".into(),
            content: serde_json::json!({"config": true}),
        };
        let result = resource.read().await.unwrap();
        assert_eq!(result, serde_json::json!({"config": true}));
    }
}
