//! Platform and product configuration types.
//!
//! Platform config (konf.toml + env vars) is loaded once at startup.
//! Product config (tools.yaml, workflows/, prompts/) is hot-reloadable.
#![allow(missing_docs)] // Config structs are self-documenting via field names

use std::path::PathBuf;

use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Top-level platform configuration.
/// Loaded from: serde defaults → konf.toml → KONF_* env vars.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PlatformConfig {
    pub database: Option<DatabaseConfig>,
    pub auth: AuthConfig,
    pub server: ServerConfig,
    pub engine: konflux_substrate::EngineConfig,
    pub runtime: konf_runtime::scope::ResourceLimits,
    pub mcp_enabled: bool,
    pub config_dir: PathBuf,
}

impl Default for PlatformConfig {
    fn default() -> Self {
        Self {
            database: None,
            auth: AuthConfig::default(),
            server: ServerConfig::default(),
            engine: konflux_substrate::EngineConfig::default(),
            runtime: konf_runtime::scope::ResourceLimits::default(),
            mcp_enabled: false,
            config_dir: PathBuf::from("./config"),
        }
    }
}

impl PlatformConfig {
    /// Load config: serde defaults → konf.toml → env vars (KONF_ prefix).
    pub fn load(config_dir: &std::path::Path) -> Result<Self, Box<figment::Error>> {
        let toml_path = config_dir.join("konf.toml");
        Figment::new()
            .merge(Toml::file(toml_path))
            .merge(Env::prefixed("KONF_").split("__"))
            .extract()
            .map_err(Box::new)
    }

    /// Validate all config sections. Returns errors if invalid.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if let Err(e) = self.engine.validate() {
            errors.push(format!("engine: {e}"));
        }
        if let Err(e) = self.runtime.validate() {
            errors.push(format!("runtime: {e}"));
        }
        if self.server.port == 0 {
            errors.push("server.port must be > 0".into());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Persistent-storage settings. Optional — edge deployments have no state.
///
/// The journal, scheduler, and runner intents are all backed by a single
/// redb database file. `url` accepts:
/// - `redb:///absolute/path/konf.redb`
/// - `file:///absolute/path/konf.redb`
/// - a bare path (relative or absolute)
///
/// Debug impl redacts the URL to prevent path/credential leakage in logs.
#[derive(Clone, Deserialize)]
pub struct DatabaseConfig {
    /// Path or URL to the redb file.
    pub url: String,
    /// Retention window in days for journal entries and terminated runner
    /// intents. Defaults to 7.
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
}

impl std::fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("url", &"[REDACTED]")
            .field("retention_days", &self.retention_days)
            .finish()
    }
}

fn default_retention_days() -> u32 {
    7
}

/// Auth settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    pub supabase_url: String,
    pub jwt_audience: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            supabase_url: "http://localhost:9999".into(),
            jwt_audience: "authenticated".into(),
        }
    }
}

/// HTTP server settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    /// Allowed CORS origins. Empty = allow all (dev only). In production, set explicitly.
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".into(),
            port: 8000,
            cors_origins: Vec::new(), // empty = allow all (dev default)
        }
    }
}

/// Product configuration — tools, workflows, prompts, guards. Hot-reloadable.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ProductConfig {
    pub tools: ToolsConfig,
    pub workflows: Vec<String>, // paths to workflow YAML files
    pub prompts: Vec<String>,   // paths to prompt template files
    pub tool_guards: std::collections::HashMap<String, ToolGuardConfig>,
    pub roles: std::collections::HashMap<String, RoleConfig>,
}

/// Product identity and entry points from project.yaml.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProjectConfig {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub triggers: std::collections::HashMap<String, TriggerConfig>,
}

fn default_version() -> String {
    "0.1.0".into()
}

/// A trigger maps an entry point to a workflow and capabilities.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TriggerConfig {
    pub workflow: String,
    pub capabilities: Vec<String>,
}

/// Guard configuration for a single tool. Evaluated at registry construction time.
///
/// # YAML example
///
/// ```yaml
/// tool_guards:
///   shell_exec:
///     rules:
///       - deny:
///           predicate:
///             contains: { path: "command", value: "sudo" }
///           message: "sudo is not allowed"
///       - deny:
///           predicate:
///             matches: { path: "command", pattern: "rm -rf*" }
///           message: "destructive rm blocked"
///     default: allow
///   dangerous_tool:
///     alias: workflow_safe_dangerous_tool
/// ```
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ToolGuardConfig {
    /// Ordered deny/allow rules. First match wins.
    pub rules: Vec<konf_runtime::guard::Rule>,
    /// Behavior when no rule matches. Default: allow.
    pub default: konf_runtime::guard::DefaultAction,
    /// Optional: redirect calls to this tool to a wrapper workflow instead.
    /// The wrapper workflow must be registered as a tool (register_as_tool: true).
    pub alias: Option<String>,
}

/// Role definition for capability scoping. Maps role name → capabilities + namespace.
///
/// # YAML example
///
/// ```yaml
/// roles:
///   admin:
///     capabilities: ["*"]
///   agent:
///     capabilities: ["memory:*", "ai:complete", "workflow:*"]
///     namespace_suffix: "agents"
///   guest:
///     capabilities: ["echo", "template"]
///     namespace_suffix: "guest"
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RoleConfig {
    /// Capability patterns granted to this role.
    pub capabilities: Vec<String>,
    /// Optional namespace suffix appended to the product namespace.
    #[serde(default)]
    pub namespace_suffix: Option<String>,
    /// Optional resource limit overrides for this role.
    #[serde(default)]
    pub limits: Option<konf_runtime::scope::ResourceLimits>,
}

/// Tools configuration from tools.yaml.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ToolsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_servers: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<ShellConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretConfig>,
}

pub use konf_tool_secret::SecretConfig;

/// Shell sandbox configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ShellConfig {
    /// Docker container name for shell_exec.
    pub container: String,
    /// Default per-command timeout in milliseconds.
    #[serde(default = "default_shell_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_shell_timeout_ms() -> u64 {
    30000
}

/// Memory backend configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemoryConfig {
    pub backend: String,
    #[serde(default)]
    pub config: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_config_parsing() {
        let yaml = r#"
secret:
  allowed_keys:
    - "STRIPE_SECRET_KEY"
    - "ANTHROPIC_API_KEY"
"#;
        let tools: ToolsConfig = serde_yaml::from_str(yaml).unwrap();
        let secret = tools.secret.expect("SecretConfig should not be None");
        assert_eq!(
            secret.allowed_keys,
            vec!["STRIPE_SECRET_KEY", "ANTHROPIC_API_KEY"]
        );
    }

    #[test]
    fn test_zero_port_fails_validation() {
        let mut config = PlatformConfig::default();
        config.server.port = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_database_is_optional() {
        let config = PlatformConfig::default();
        assert!(config.database.is_none());
    }

    #[test]
    fn test_product_config_default_has_no_guards() {
        let config = ProductConfig::default();
        assert!(config.tool_guards.is_empty());
        assert!(config.roles.is_empty());
    }

    #[test]
    fn test_tool_guards_deserialize_from_yaml() {
        let yaml = r#"
tool_guards:
  "shell:exec":
    rules:
      - action: deny
        predicate:
          type: contains
          path: "command"
          value: "sudo"
        message: "sudo blocked"
      - action: deny
        predicate:
          type: matches
          path: "command"
          pattern: "rm -rf*"
        message: "destructive rm blocked"
    default: deny
  echo:
    alias: "workflow:safe_echo"
"#;
        let config: ProductConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.tool_guards.len(), 2);

        let shell_guard = &config.tool_guards["shell:exec"];
        assert_eq!(shell_guard.rules.len(), 2);
        assert_eq!(
            shell_guard.default,
            konf_runtime::guard::DefaultAction::Deny
        );
        assert!(shell_guard.alias.is_none());

        let echo_guard = &config.tool_guards["echo"];
        assert_eq!(echo_guard.alias.as_deref(), Some("workflow:safe_echo"));
    }

    #[test]
    fn test_roles_deserialize_from_yaml() {
        let yaml = r#"
roles:
  admin:
    capabilities: ["*"]
  agent:
    capabilities: ["memory:*", "ai:complete"]
    namespace_suffix: "agents"
"#;
        let config: ProductConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.roles.len(), 2);

        let admin = &config.roles["admin"];
        assert_eq!(admin.capabilities, vec!["*"]);
        assert!(admin.namespace_suffix.is_none());

        let agent = &config.roles["agent"];
        assert_eq!(agent.capabilities, vec!["memory:*", "ai:complete"]);
        assert_eq!(agent.namespace_suffix.as_deref(), Some("agents"));
    }

    #[test]
    fn test_tool_guards_absent_defaults_to_empty() {
        let yaml = r#"
tools:
  shell:
    container: "konf-sandbox"
"#;
        let config: ProductConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.tool_guards.is_empty());
        assert!(config.roles.is_empty());
    }
}
