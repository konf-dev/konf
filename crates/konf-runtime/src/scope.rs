//! Execution scope — capabilities, resource limits, and actor identity.
//!
//! Every workflow run is scoped to a namespace with specific capability
//! grants. The capability lattice ensures children can only attenuate,
//! never amplify.
//!
//! `ExecutionScope` carries **configuration**: who the actor is, what
//! they're allowed to do, and within what bounds. It does NOT carry
//! runtime state — for that see [`crate::ExecutionContext`], which
//! holds the per-dispatch `trace_id`, `parent_interaction_id`, and
//! `session_id`. This split is deliberate (Phase F2.R2 of the
//! Stigmergic Engine plan): `ExecutionScope` is immutable once
//! constructed; `ExecutionContext` mutates as dispatches nest.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::RuntimeError;

/// Defines what a workflow execution is allowed to do.
#[derive(Debug, Clone)]
pub struct ExecutionScope {
    /// Hierarchical namespace (e.g., "konf:unspool:user_123").
    pub namespace: String,
    /// Granted capabilities with parameter bindings.
    pub capabilities: Vec<CapabilityGrant>,
    /// Resource limits for this execution.
    pub limits: ResourceLimits,
    /// Identity of the actor initiating this execution.
    pub actor: Actor,
    /// Current nesting depth (0 = root workflow, incremented for child workflows).
    pub depth: usize,
}

/// A capability grant with optional parameter bindings.
///
/// The `pattern` field supports glob matching:
/// - `"memory:*"` matches `"memory:search"`, `"memory:store"`
/// - `"ai:complete"` matches exactly
/// - `"*"` matches everything
///
/// The `bindings` field contains parameters injected into tool input,
/// overriding any LLM-set values. Key use: namespace injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityGrant {
    pub pattern: String,
    #[serde(default)]
    pub bindings: HashMap<String, Value>,
}

impl CapabilityGrant {
    /// Create a grant with no bindings.
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            bindings: HashMap::new(),
        }
    }

    /// Create a grant with bindings.
    pub fn with_bindings(pattern: impl Into<String>, bindings: HashMap<String, Value>) -> Self {
        Self {
            pattern: pattern.into(),
            bindings,
        }
    }

    /// Check if this grant matches a tool name. Returns bindings if matched.
    pub fn matches(&self, tool_name: &str) -> Option<&HashMap<String, Value>> {
        if matches_capability_pattern(&self.pattern, tool_name) {
            Some(&self.bindings)
        } else {
            None
        }
    }
}

/// Check if a capability pattern matches a tool name.
/// Uses the same logic as konflux::capability::matches_capability.
fn matches_capability_pattern(pattern: &str, tool_name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix(":*") {
        // "memory:*" matches "memory:search" but not "memorysearch"
        return tool_name.starts_with(prefix)
            && tool_name.get(prefix.len()..prefix.len() + 1) == Some(":");
    }
    pattern == tool_name
}

impl ExecutionScope {
    /// Check if a tool is allowed and return its parameter bindings.
    pub fn check_tool(&self, tool_name: &str) -> Result<HashMap<String, Value>, RuntimeError> {
        for grant in &self.capabilities {
            if let Some(bindings) = grant.matches(tool_name) {
                return Ok(bindings.clone());
            }
        }
        Err(RuntimeError::CapabilityDenied(format!(
            "tool '{}' not granted in scope '{}'",
            tool_name, self.namespace
        )))
    }

    /// Create a child scope with attenuated capabilities.
    /// Validates that all child capability patterns are a subset of the parent's grants.
    ///
    /// Note: bindings are NOT validated against the parent because child_scope()
    /// is called by the runtime (not the LLM). Bindings come from product config
    /// and are trusted. The LLM never controls bindings directly — VirtualizedTool
    /// injects them before the tool sees the input.
    pub fn child_scope(
        &self,
        child_capabilities: Vec<CapabilityGrant>,
        child_namespace: Option<String>,
    ) -> Result<ExecutionScope, RuntimeError> {
        // Every child capability must be covered by a parent capability.
        // A child pattern is valid if it is equal to or more specific than a parent pattern.
        for child_grant in &child_capabilities {
            let covered = self.capabilities.iter().any(|parent_grant| {
                if parent_grant.pattern == "*" {
                    return true;
                }
                if parent_grant.pattern == child_grant.pattern {
                    return true;
                }
                // A parent prefix (e.g. "memory:*") covers a child-specific tool (e.g. "memory:search")
                // but NOT a child prefix (e.g. "memory:*" cannot be granted if parent only has "memory:search")
                if let Some(parent_prefix) = parent_grant.pattern.strip_suffix(":*") {
                    // Child is a specific tool under the parent prefix
                    if !child_grant.pattern.ends_with(":*") {
                        return matches_capability_pattern(
                            &parent_grant.pattern,
                            &child_grant.pattern,
                        );
                    }
                    // Child is also a prefix — must be equal or more specific
                    if let Some(child_prefix) = child_grant.pattern.strip_suffix(":*") {
                        return child_prefix.starts_with(parent_prefix)
                            && (child_prefix == parent_prefix
                                || child_prefix.as_bytes().get(parent_prefix.len())
                                    == Some(&b':'));
                    }
                }
                false
            });
            if !covered {
                return Err(RuntimeError::CapabilityDenied(format!(
                    "capability '{}' cannot be granted — parent scope '{}' does not have it",
                    child_grant.pattern, self.namespace
                )));
            }
        }

        Ok(ExecutionScope {
            namespace: child_namespace.unwrap_or_else(|| self.namespace.clone()),
            capabilities: child_capabilities,
            limits: self.limits.clone(),
            actor: self.actor.clone(),
            depth: self.depth + 1,
        })
    }

    /// Validate that starting a new run is allowed given current state.
    pub fn validate_start(&self, table: &crate::ProcessTable) -> Result<(), RuntimeError> {
        // Check concurrent runs limit
        let active = table.active_count_in_namespace(&self.namespace);
        if active >= self.limits.max_active_runs_per_namespace {
            return Err(RuntimeError::ResourceLimit {
                limit: "max_active_runs_per_namespace".into(),
                value: self.limits.max_active_runs_per_namespace,
            });
        }
        // Check child depth limit
        if self.depth >= self.limits.max_child_depth {
            return Err(RuntimeError::ResourceLimit {
                limit: "max_child_depth".into(),
                value: self.limits.max_child_depth,
            });
        }
        Ok(())
    }

    /// Get the list of capability patterns (for passing to konflux engine).
    pub fn capability_patterns(&self) -> Vec<String> {
        self.capabilities
            .iter()
            .map(|g| g.pattern.clone())
            .collect()
    }
}

/// Resource limits for a workflow execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum steps per workflow (default: 1000).
    pub max_steps: usize,
    /// Maximum workflow duration in ms (default: 300_000 = 5 min).
    pub max_workflow_timeout_ms: u64,
    /// Maximum concurrent nodes per workflow (default: 50).
    pub max_concurrent_nodes: usize,
    /// Maximum nesting depth for child workflows (default: 10).
    pub max_child_depth: usize,
    /// Maximum concurrent runs per namespace (default: 20).
    pub max_active_runs_per_namespace: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_steps: 1000,
            max_workflow_timeout_ms: 300_000,
            max_concurrent_nodes: 50,
            max_child_depth: 10,
            max_active_runs_per_namespace: 20,
        }
    }
}

impl ResourceLimits {
    /// Validate that all limits are sane (non-zero safety limits).
    pub fn validate(&self) -> Result<(), String> {
        if self.max_steps == 0 {
            return Err("max_steps must be > 0 (prevents infinite loops)".into());
        }
        if self.max_workflow_timeout_ms == 0 {
            return Err("max_workflow_timeout_ms must be > 0 (prevents runaway workflows)".into());
        }
        if self.max_concurrent_nodes == 0 {
            return Err("max_concurrent_nodes must be > 0".into());
        }
        if self.max_child_depth == 0 {
            return Err("max_child_depth must be > 0".into());
        }
        if self.max_active_runs_per_namespace == 0 {
            return Err("max_active_runs_per_namespace must be > 0".into());
        }
        Ok(())
    }
}

/// Identity of the actor executing a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Actor {
    pub id: String,
    pub role: ActorRole,
}

/// Role of the actor. Serialized as snake_case in JSON/SQL.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActorRole {
    InfraAdmin,
    ProductAdmin,
    User,
    InfraAgent,
    ProductAgent,
    UserAgent,
    System,
}

/// Build an `ExecutionScope` from a role name and product context.
///
/// This is the shared auth resolution path used by both HTTP (axum middleware)
/// and MCP session setup. The caller resolves their auth mechanism (JWT, API key,
/// etc.) to a role string, then calls this function.
///
/// # Arguments
///
/// * `actor_id` — Unique identifier for the actor (e.g., user ID, bot name)
/// * `role_name` — Role string from auth (e.g., "admin", "agent", "guest")
/// * `product_namespace` — Base namespace for the product (e.g., "konf:unspool")
/// * `role_capabilities` — Capability patterns for this role
/// * `namespace_suffix` — Optional suffix appended to product namespace
/// * `limits` — Resource limits (or default)
pub fn scope_from_role(
    actor_id: impl Into<String>,
    role_name: &str,
    product_namespace: &str,
    role_capabilities: &[String],
    namespace_suffix: Option<&str>,
    limits: ResourceLimits,
) -> ExecutionScope {
    let actor_role = match role_name {
        "infra_admin" => ActorRole::InfraAdmin,
        "product_admin" | "admin" => ActorRole::ProductAdmin,
        "user" => ActorRole::User,
        "infra_agent" => ActorRole::InfraAgent,
        "product_agent" | "agent" => ActorRole::ProductAgent,
        "user_agent" => ActorRole::UserAgent,
        "system" => ActorRole::System,
        _ => ActorRole::User, // safe default
    };

    let namespace = match namespace_suffix {
        Some(suffix) => format!("{product_namespace}:{suffix}"),
        None => product_namespace.to_string(),
    };

    let capabilities = role_capabilities
        .iter()
        .map(|pattern| CapabilityGrant::new(pattern.clone()))
        .collect();

    ExecutionScope {
        namespace,
        capabilities,
        limits,
        actor: Actor {
            id: actor_id.into(),
            role: actor_role,
        },
        depth: 0,
    }
}

/// Build a dev-mode `ExecutionScope` with full access. Used when no auth is configured.
pub fn dev_scope(product_namespace: &str) -> ExecutionScope {
    scope_from_role(
        "dev_user",
        "infra_admin",
        product_namespace,
        &["*".to_string()],
        None,
        ResourceLimits::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_exact_match() {
        let grant = CapabilityGrant::new("memory:search");
        assert!(grant.matches("memory:search").is_some());
        assert!(grant.matches("memory:store").is_none());
    }

    #[test]
    fn test_capability_glob_match() {
        let grant = CapabilityGrant::new("memory:*");
        assert!(grant.matches("memory:search").is_some());
        assert!(grant.matches("memory:store").is_some());
        // Must have underscore separator — "memorysearch" should NOT match
        assert!(grant.matches("memorysearch").is_none());
    }

    #[test]
    fn test_capability_wildcard_all() {
        let grant = CapabilityGrant::new("*");
        assert!(grant.matches("anything").is_some());
        assert!(grant.matches("memory:search").is_some());
    }

    #[test]
    fn test_capability_bindings_returned() {
        let mut bindings = HashMap::new();
        bindings.insert(
            "namespace".to_string(),
            Value::String("user_123".to_string()),
        );
        let grant = CapabilityGrant::with_bindings("memory:*", bindings);

        let result = grant.matches("memory:search").unwrap();
        assert_eq!(result.get("namespace").unwrap(), "user_123");
    }

    #[test]
    fn test_scope_check_tool_allowed() {
        let scope = ExecutionScope {
            namespace: "konf:test:user_1".into(),
            capabilities: vec![
                CapabilityGrant::new("memory:search"),
                CapabilityGrant::new("ai:complete"),
            ],
            limits: ResourceLimits::default(),
            actor: Actor {
                id: "user_1".into(),
                role: ActorRole::User,
            },
            depth: 0,
            };

        assert!(scope.check_tool("memory:search").is_ok());
        assert!(scope.check_tool("ai:complete").is_ok());
        assert!(scope.check_tool("memory:store").is_err());
    }

    #[test]
    fn test_scope_check_tool_with_bindings() {
        let mut bindings = HashMap::new();
        bindings.insert(
            "namespace".to_string(),
            Value::String("konf:test:user_1".to_string()),
        );

        let scope = ExecutionScope {
            namespace: "konf:test:user_1".into(),
            capabilities: vec![CapabilityGrant::with_bindings("memory:*", bindings)],
            limits: ResourceLimits::default(),
            actor: Actor {
                id: "user_1".into(),
                role: ActorRole::User,
            },
            depth: 0,
            };

        let result = scope.check_tool("memory:search").unwrap();
        assert_eq!(result.get("namespace").unwrap(), "konf:test:user_1");
    }

    #[test]
    fn test_child_scope_validates_subset() {
        let parent = ExecutionScope {
            namespace: "konf:test".into(),
            capabilities: vec![
                CapabilityGrant::new("memory:*"),
                CapabilityGrant::new("ai:complete"),
            ],
            limits: ResourceLimits::default(),
            actor: Actor {
                id: "admin".into(),
                role: ActorRole::ProductAdmin,
            },
            depth: 0,
            };

        // Valid child — subset of parent
        let child = parent.child_scope(
            vec![CapabilityGrant::new("memory:search")],
            Some("konf:test:user_1".into()),
        );
        assert!(child.is_ok());

        // Invalid child — http_get not in parent
        let child = parent.child_scope(vec![CapabilityGrant::new("http:get")], None);
        assert!(child.is_err());
    }

    #[test]
    fn test_child_scope_cannot_amplify() {
        let parent = ExecutionScope {
            namespace: "konf:test".into(),
            capabilities: vec![CapabilityGrant::new("memory:search")],
            limits: ResourceLimits::default(),
            actor: Actor {
                id: "admin".into(),
                role: ActorRole::ProductAdmin,
            },
            depth: 0,
            };

        // Child cannot escalate "memory:search" to "memory:*"
        let child = parent.child_scope(vec![CapabilityGrant::new("memory:*")], None);
        assert!(
            child.is_err(),
            "child should not be able to amplify capability"
        );

        // Child cannot escalate to wildcard
        let child = parent.child_scope(vec![CapabilityGrant::new("*")], None);
        assert!(
            child.is_err(),
            "child should not be able to get wildcard from specific grant"
        );
    }

    #[test]
    fn test_child_scope_prefix_attenuation() {
        let parent = ExecutionScope {
            namespace: "konf:test".into(),
            capabilities: vec![CapabilityGrant::new("memory:*")],
            limits: ResourceLimits::default(),
            actor: Actor {
                id: "admin".into(),
                role: ActorRole::ProductAdmin,
            },
            depth: 0,
            };

        // Same prefix is allowed
        let child = parent.child_scope(vec![CapabilityGrant::new("memory:*")], None);
        assert!(child.is_ok());

        // Specific tool under prefix is allowed
        let child = parent.child_scope(vec![CapabilityGrant::new("memory:search")], None);
        assert!(child.is_ok());
    }

    #[test]
    fn test_actor_role_serialization() {
        let role = ActorRole::InfraAdmin;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"infra_admin\"");

        let role: ActorRole = serde_json::from_str("\"user_agent\"").unwrap();
        assert_eq!(role, ActorRole::UserAgent);
    }

    #[test]
    fn test_scope_from_role_admin() {
        let scope = scope_from_role(
            "alice",
            "admin",
            "konf:myproduct",
            &["*".to_string()],
            None,
            ResourceLimits::default(),
        );
        assert_eq!(scope.namespace, "konf:myproduct");
        assert_eq!(scope.actor.id, "alice");
        assert_eq!(scope.actor.role, ActorRole::ProductAdmin);
        assert_eq!(scope.capabilities.len(), 1);
        assert!(scope.capabilities[0].matches("anything").is_some());
    }

    #[test]
    fn test_scope_from_role_with_suffix() {
        let scope = scope_from_role(
            "bot_1",
            "agent",
            "konf:myproduct",
            &["memory:*".to_string(), "ai:complete".to_string()],
            Some("agents"),
            ResourceLimits::default(),
        );
        assert_eq!(scope.namespace, "konf:myproduct:agents");
        assert_eq!(scope.actor.role, ActorRole::ProductAgent);
        assert_eq!(scope.capabilities.len(), 2);
    }

    #[test]
    fn test_scope_from_role_unknown_defaults_to_user() {
        let scope = scope_from_role(
            "unknown",
            "some_random_role",
            "konf:test",
            &[],
            None,
            ResourceLimits::default(),
        );
        assert_eq!(scope.actor.role, ActorRole::User);
    }

    #[test]
    fn test_dev_scope_has_full_access() {
        let scope = dev_scope("konf:dev");
        assert_eq!(scope.actor.id, "dev_user");
        assert_eq!(scope.actor.role, ActorRole::InfraAdmin);
        assert!(scope.capabilities[0].matches("anything").is_some());
    }
}
