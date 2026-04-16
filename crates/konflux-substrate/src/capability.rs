//! Capability lattice — capabilities only attenuate, never amplify.
//!
//! The typed `Capability` and `CapSet` types in `crate::envelope` own
//! the matching and attenuation logic. These free functions are thin
//! legacy wrappers for callers that still pass `&[String]`.

use crate::envelope::CapSet;
use crate::error::ToolError;

/// Check if a tool invocation is allowed given string capability patterns.
///
/// Prefer `CapSet::check_access()` when you already have a `CapSet`.
pub fn check_tool_access(tool_name: &str, capabilities: &[String]) -> Result<(), ToolError> {
    CapSet::from_patterns(capabilities).check_access(tool_name)
}

/// Check if a grant list is a subset of the parent's capabilities.
///
/// Prefer `CapSet::attenuate()` when you already have a `CapSet`.
pub fn validate_grant(grant: &[String], parent_capabilities: &[String]) -> Result<(), String> {
    CapSet::from_patterns(parent_capabilities)
        .attenuate(grant)
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_tool_access_deny_empty() {
        assert!(check_tool_access("echo", &[]).is_err());
    }

    #[test]
    fn check_tool_access_exact_match() {
        assert!(check_tool_access("echo", &["echo".to_string()]).is_ok());
    }

    #[test]
    fn check_tool_access_wildcard() {
        assert!(check_tool_access("echo", &["*".to_string()]).is_ok());
    }

    #[test]
    fn check_tool_access_prefix_match() {
        assert!(check_tool_access("memory:search", &["memory:*".to_string()]).is_ok());
        assert!(check_tool_access("memorysearch", &["memory:*".to_string()]).is_err());
    }

    #[test]
    fn check_tool_access_denial() {
        assert!(check_tool_access("fail", &["echo".to_string()]).is_err());
    }

    #[test]
    fn validate_grant_empty_child() {
        assert!(validate_grant(&[], &["*".to_string()]).is_ok());
    }

    #[test]
    fn validate_grant_empty_parent() {
        assert!(validate_grant(&["echo".to_string()], &[]).is_err());
    }

    #[test]
    fn validate_grant_subset() {
        assert!(validate_grant(&["mem:s".into()], &["mem:*".into()]).is_ok());
    }

    #[test]
    fn validate_grant_not_subset() {
        assert!(validate_grant(&["other".into()], &["mem:*".into()]).is_err());
    }
}
