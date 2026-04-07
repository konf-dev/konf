//! Platform and product configuration types.
//!
//! Platform config (konf.toml + env vars) is loaded once at startup.
//! Product config (tools.yaml, workflows/, prompts/) is hot-reloadable.
#![allow(missing_docs)] // Config structs are self-documenting via field names

use std::path::PathBuf;

use figment::{Figment, providers::{Env, Format, Toml}};
use serde::Deserialize;
use serde_json::Value;

/// Top-level platform configuration.
/// Loaded from: serde defaults → konf.toml → KONF_* env vars.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PlatformConfig {
    pub database: Option<DatabaseConfig>,
    pub auth: AuthConfig,
    pub server: ServerConfig,
    pub engine: konflux::EngineConfig,
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
            engine: konflux::EngineConfig::default(),
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

        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}

/// Database connection settings. Optional — edge deployments have no DB.
/// Debug impl redacts the URL to prevent credential leakage in logs.
#[derive(Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    #[serde(default = "default_pool_min")]
    pub pool_min: u32,
    #[serde(default = "default_pool_max")]
    pub pool_max: u32,
}

impl std::fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("url", &"[REDACTED]")
            .field("pool_min", &self.pool_min)
            .field("pool_max", &self.pool_max)
            .finish()
    }
}

fn default_pool_min() -> u32 { 5 }
fn default_pool_max() -> u32 { 20 }

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

/// Product configuration — tools, workflows, prompts. Hot-reloadable.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProductConfig {
    pub tools: ToolsConfig,
    pub workflows: Vec<String>,  // paths to workflow YAML files
    pub prompts: Vec<String>,    // paths to prompt template files
}

/// Tools configuration from tools.yaml.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    pub memory: Option<MemoryConfig>,
    pub llm: Option<Value>,
    pub http: Option<Value>,
    pub embed: Option<Value>,
    pub mcp_servers: Option<Value>,
}

/// Memory backend configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct MemoryConfig {
    pub backend: String,
    #[serde(default)]
    pub config: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_valid() {
        let config = PlatformConfig::default();
        assert!(config.validate().is_ok());
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
}
