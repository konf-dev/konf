//! HTTP tools (http:get, http:post) for the Konf platform.
//!
//! Wraps reqwest for making HTTP requests. Both tools are capped at
//! MAX_TIMEOUT_SECS to prevent resource exhaustion from LLM-controlled timeouts.
#![warn(missing_docs)]

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{debug, warn};

use konflux::error::ToolError;
use konflux::tool::{Tool, ToolAnnotations, ToolContext, ToolInfo};
use konflux::Engine;

/// Maximum timeout for HTTP requests (5 minutes) to prevent resource exhaustion.
const MAX_TIMEOUT_SECS: u64 = 300;

/// Maximum response body size (10 MB) to prevent memory exhaustion.
const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

/// Register http:get and http:post tools in the engine.
pub async fn register(engine: &Engine, _config: &Value) -> anyhow::Result<()> {
    engine.register_tool(Arc::new(HttpGetTool::new()));
    engine.register_tool(Arc::new(HttpPostTool::new()));
    Ok(())
}

/// Validate a URL is safe to fetch (prevent SSRF against internal services).
fn validate_url(url: &str) -> Result<(), ToolError> {
    let parsed = url::Url::parse(url).map_err(|e| ToolError::InvalidInput {
        message: format!("Invalid URL: {e}"),
        field: Some("url".into()),
    })?;

    // Only allow http/https schemes
    match parsed.scheme() {
        "http" | "https" => {}
        scheme => return Err(ToolError::InvalidInput {
            message: format!("Unsupported URL scheme: {scheme}"),
            field: Some("url".into()),
        }),
    }

    // Block internal/private IP ranges
    if let Some(host) = parsed.host() {
        match host {
            url::Host::Domain(domain) => {
                let blocked = ["localhost", "metadata.google.internal"];
                if blocked.contains(&domain) {
                    return Err(ToolError::InvalidInput {
                        message: format!("URL host '{domain}' is blocked"),
                        field: Some("url".into()),
                    });
                }
            }
            url::Host::Ipv4(ip) => {
                // Block: loopback, private (RFC1918), link-local (169.254.x.x including AWS IMDS), unspecified
                if ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified() {
                    return Err(ToolError::InvalidInput {
                        message: format!("URL host '{ip}' is a private/internal IP"),
                        field: Some("url".into()),
                    });
                }
            }
            url::Host::Ipv6(ip) => {
                if ip.is_loopback() || ip.segments()[0] == 0xfe80 {
                    return Err(ToolError::InvalidInput {
                        message: format!("URL host '{ip}' is a private/internal IP"),
                        field: Some("url".into()),
                    });
                }
            }
        }
    }

    Ok(())
}

/// Read response body with size limit to prevent memory exhaustion.
async fn read_body_limited(resp: reqwest::Response) -> Result<String, ToolError> {
    let content_length = resp.content_length().unwrap_or(0) as usize;
    if content_length > MAX_RESPONSE_BYTES {
        return Err(ToolError::ExecutionFailed {
            message: format!("Response too large: {content_length} bytes (max {MAX_RESPONSE_BYTES})"),
            retryable: false,
        });
    }
    let bytes = resp.bytes().await.map_err(tool_err)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(ToolError::ExecutionFailed {
            message: format!("Response too large: {} bytes (max {MAX_RESPONSE_BYTES})", bytes.len()),
            retryable: false,
        });
    }
    String::from_utf8(bytes.to_vec()).map_err(|e| ToolError::ExecutionFailed {
        message: format!("Response is not valid UTF-8: {e}"),
        retryable: false,
    })
}

fn tool_err(e: impl std::fmt::Display) -> ToolError {
    ToolError::ExecutionFailed {
        message: e.to_string(),
        retryable: true,
    }
}

// ============================================================
// http:get
// ============================================================

/// HTTP GET tool — makes a GET request and returns status, headers, body.
pub struct HttpGetTool {
    client: reqwest::Client,
}

impl HttpGetTool {
    /// Create a new HttpGetTool with a default reqwest client.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Tool for HttpGetTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "http:get".into(),
            description: "Make an HTTP GET request and return the response.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to fetch" },
                    "headers": { "type": "object", "description": "Optional headers" },
                    "timeout": { "type": "integer", "description": "Timeout in seconds", "default": 30 }
                },
                "required": ["url"]
            }),
            output_schema: None,
            capabilities: vec!["http:get".into()],
            supports_streaming: false,
            annotations: ToolAnnotations { open_world: true, idempotent: true, ..Default::default() },
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let url = input.get("url").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput { message: "missing 'url'".into(), field: Some("url".into()) })?;
        validate_url(url)?;

        let timeout_secs = input.get("timeout").and_then(|v| v.as_u64()).unwrap_or(30).min(MAX_TIMEOUT_SECS);

        let mut req = self.client.get(url)
            .timeout(std::time::Duration::from_secs(timeout_secs));

        if let Some(headers) = input.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in headers {
                if let Some(val) = v.as_str() {
                    req = req.header(k.as_str(), val);
                }
            }
        }

        let start = std::time::Instant::now();
        let resp = req.send().await.map_err(|e| {
            warn!(url = %url, error = %e, "HTTP GET failed");
            tool_err(e)
        })?;

        let status = resp.status().as_u16();
        let headers: serde_json::Map<String, Value> = resp.headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str().ok().map(|s| (k.to_string(), Value::String(s.to_string())))
            })
            .collect();

        let body = read_body_limited(resp).await?;
        let duration_ms = start.elapsed().as_millis() as u64;
        debug!(url = %url, status, duration_ms, "HTTP GET completed");

        let body_value = serde_json::from_str::<Value>(&body).unwrap_or(Value::String(body));

        Ok(json!({
            "status": status,
            "headers": headers,
            "body": body_value,
            "_meta": {
                "tool": "http:get",
                "duration_ms": duration_ms,
            }
        }))
    }
}

// ============================================================
// http:post
// ============================================================

/// HTTP POST tool — makes a POST request with a JSON body.
pub struct HttpPostTool {
    client: reqwest::Client,
}

impl HttpPostTool {
    /// Create a new HttpPostTool with a default reqwest client.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Tool for HttpPostTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "http:post".into(),
            description: "Make an HTTP POST request with a JSON body.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to post to" },
                    "body": { "description": "JSON body to send" },
                    "headers": { "type": "object", "description": "Optional headers" },
                    "timeout": { "type": "integer", "description": "Timeout in seconds", "default": 30 }
                },
                "required": ["url"]
            }),
            output_schema: None,
            capabilities: vec!["http:post".into()],
            supports_streaming: false,
            annotations: ToolAnnotations { open_world: true, ..Default::default() },
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let url = input.get("url").and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput { message: "missing 'url'".into(), field: Some("url".into()) })?;
        validate_url(url)?;

        let timeout_secs = input.get("timeout").and_then(|v| v.as_u64()).unwrap_or(30).min(MAX_TIMEOUT_SECS);
        let body = input.get("body").cloned().unwrap_or(Value::Null);

        let mut req = self.client.post(url)
            .json(&body)
            .timeout(std::time::Duration::from_secs(timeout_secs));

        if let Some(headers) = input.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in headers {
                if let Some(val) = v.as_str() {
                    req = req.header(k.as_str(), val);
                }
            }
        }

        let start = std::time::Instant::now();
        let resp = req.send().await.map_err(|e| {
            warn!(url = %url, error = %e, "HTTP POST failed");
            tool_err(e)
        })?;

        let status = resp.status().as_u16();
        let resp_body = read_body_limited(resp).await?;
        let duration_ms = start.elapsed().as_millis() as u64;
        debug!(url = %url, status, duration_ms, "HTTP POST completed");

        let body_value = serde_json::from_str::<Value>(&resp_body).unwrap_or(Value::String(resp_body));

        Ok(json!({
            "status": status,
            "body": body_value,
            "_meta": {
                "tool": "http:post",
                "duration_ms": duration_ms,
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_get_tool_info() {
        let tool = HttpGetTool::new();
        let info = tool.info();
        assert_eq!(info.name, "http:get");
        assert!(info.annotations.open_world);
        assert!(info.annotations.idempotent);
        assert!(!info.annotations.destructive);
    }

    #[test]
    fn test_http_post_tool_info() {
        let tool = HttpPostTool::new();
        let info = tool.info();
        assert_eq!(info.name, "http:post");
        assert!(info.annotations.open_world);
        assert!(!info.annotations.idempotent);
    }

    #[test]
    fn test_register_adds_both_tools() {
        let engine = Engine::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(register(&engine, &json!({}))).unwrap();
        assert!(engine.registry().contains("http:get"));
        assert!(engine.registry().contains("http:post"));
        assert_eq!(engine.registry().len(), 2);
    }

    // ---- SSRF validation tests ----

    #[test]
    fn test_validate_url_allows_public_https() {
        assert!(validate_url("https://api.example.com/data").is_ok());
        assert!(validate_url("http://example.com").is_ok());
        assert!(validate_url("https://8.8.8.8/dns").is_ok());
    }

    #[test]
    fn test_validate_url_blocks_non_http_schemes() {
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("ftp://evil.com/file").is_err());
        assert!(validate_url("gopher://internal:25").is_err());
        assert!(validate_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn test_validate_url_blocks_localhost() {
        assert!(validate_url("http://localhost/admin").is_err());
        assert!(validate_url("http://127.0.0.1/secret").is_err());
        assert!(validate_url("http://0.0.0.0/").is_err());
        assert!(validate_url("http://[::1]/").is_err());
    }

    #[test]
    fn test_validate_url_blocks_aws_metadata() {
        assert!(validate_url("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(validate_url("http://metadata.google.internal/computeMetadata").is_err());
    }

    #[test]
    fn test_validate_url_blocks_private_ips() {
        // RFC 1918 ranges
        assert!(validate_url("http://10.0.0.1/internal").is_err());
        assert!(validate_url("http://172.16.0.1/private").is_err());
        assert!(validate_url("http://192.168.1.1/router").is_err());
        // Link-local
        assert!(validate_url("http://169.254.1.1/").is_err());
    }

    #[test]
    fn test_validate_url_blocks_ipv6_loopback() {
        assert!(validate_url("http://[::1]:8080/").is_err());
    }

    #[test]
    fn test_validate_url_rejects_invalid_urls() {
        assert!(validate_url("not a url").is_err());
        assert!(validate_url("").is_err());
    }

    // ---- Invoke tests (missing URL) ----

    #[tokio::test]
    async fn test_get_invoke_missing_url() {
        let tool = HttpGetTool::new();
        let ctx = ToolContext {
            capabilities: vec![],
            workflow_id: "test".into(),
            node_id: "test".into(),
            metadata: std::collections::HashMap::new(),
        };
        let result = tool.invoke(json!({}), &ctx).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ToolError::InvalidInput { field, .. } => assert_eq!(field, Some("url".into())),
            other => panic!("Expected InvalidInput, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_get_invoke_ssrf_blocked() {
        let tool = HttpGetTool::new();
        let ctx = ToolContext {
            capabilities: vec![],
            workflow_id: "test".into(),
            node_id: "test".into(),
            metadata: std::collections::HashMap::new(),
        };
        let result = tool.invoke(json!({"url": "http://169.254.169.254/latest/meta-data/"}), &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_post_invoke_missing_url() {
        let tool = HttpPostTool::new();
        let ctx = ToolContext {
            capabilities: vec![],
            workflow_id: "test".into(),
            node_id: "test".into(),
            metadata: std::collections::HashMap::new(),
        };
        let result = tool.invoke(json!({"body": {"key": "value"}}), &ctx).await;
        assert!(result.is_err());
    }
}
