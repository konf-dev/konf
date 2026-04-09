//! Capability lattice — capabilities only attenuate, never amplify.

use crate::error::ToolError;

/// Check if a tool invocation is allowed given the current capabilities.
pub fn check_tool_access(tool_name: &str, capabilities: &[String]) -> Result<(), ToolError> {
    if capabilities.is_empty() {
        // SECURITY: Empty capabilities must deny all tools.
        // Use ["*"] for unrestricted access.
        return Err(ToolError::CapabilityDenied {
            capability: "ALL (empty capability list)".to_string(),
        });
    }
    
    if capabilities.iter().any(|c| matches_capability(c, tool_name)) {
        Ok(())
    } else {
        Err(ToolError::CapabilityDenied {
            capability: tool_name.to_string(),
        })
    }
}

/// Check if a grant list is a subset of the parent's capabilities.
pub fn validate_grant(grant: &[String], parent_capabilities: &[String]) -> Result<(), String> {
    // If the child workflow/node requests no capabilities, it's always valid (full attenuation)
    if grant.is_empty() {
        return Ok(());
    }

    if parent_capabilities.is_empty() {
        return Err("Parent has no capabilities to grant from".to_string());
    }

    for cap in grant {
        if !parent_capabilities.iter().any(|p| matches_capability(p, cap)) {
            return Err(format!(
                "capability '{cap}' cannot be granted — parent does not have it"
            ));
        }
    }
    Ok(())
}

/// Match a capability pattern against a tool name.
/// Supports glob-style wildcards:
///   "memory:*" matches "memory:search", "memory:store"
///   "ai:*" matches "ai:complete", "ai_stream"
///   "*" matches everything
///   "memory:search" matches only "memory:search" (exact)
fn matches_capability(pattern: &str, tool_name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix(":*") {
        return tool_name.starts_with(prefix) && tool_name.get(prefix.len()..prefix.len()+1) == Some(":");
    }
    pattern == tool_name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_tool_access() {
        // Deny all if empty
        assert!(check_tool_access("echo", &[]).is_err());
        
        // Exact match
        assert!(check_tool_access("echo", &["echo".to_string()]).is_ok());
        
        // Wildcard match
        assert!(check_tool_access("echo", &["*".to_string()]).is_ok());
        
        // Prefix match
        assert!(check_tool_access("memory:search", &["memory:*".to_string()]).is_ok());
        assert!(check_tool_access("memorysearch", &["memory:*".to_string()]).is_err());
        
        // Denial
        assert!(check_tool_access("fail", &["echo".to_string()]).is_err());
    }

    #[test]
    fn test_validate_grant() {
        // Child requests none - always ok
        assert!(validate_grant(&[], &["*".to_string()]).is_ok());
        
        // Parent has none - deny child request
        assert!(validate_grant(&["echo".to_string()], &[]).is_err());
        
        // Subset
        assert!(validate_grant(&["mem:s".into()], &["mem:*".into()]).is_ok());

        // Not subset
        assert!(validate_grant(&["other".into()], &["mem:*".into()]).is_err());
    }
}
