//! Dependency Graph — determines execution order and parallelism.

use std::collections::{HashMap, HashSet};
use crate::parser::schema::WorkflowSchema;

/// Analyzes the workflow to build a dependency graph.
#[derive(Debug, Clone)]
pub struct DependencyGraph {
    /// Mapping of node name → list of nodes that MUST complete before it.
    dependencies: HashMap<String, HashSet<String>>,
}

impl DependencyGraph {
    /// Build a dependency graph from a workflow schema.
    pub fn build(schema: &WorkflowSchema) -> Self {
        let mut graph = Self {
            dependencies: HashMap::new(),
        };

        // Pre-initialize dependencies for all nodes
        for name in schema.nodes.keys() {
            graph.dependencies.insert(name.clone(), HashSet::new());
        }

        for (name, node) in &schema.nodes {
            // 1. Data dependencies from `with`
            // We ONLY track data dependencies in the execution graph to enable parallelism.
            // Control flow (then/catch) is handled dynamically at runtime.
            if let Some(with) = &node.with {
                let refs = extract_references(with);
                for r in refs {
                    if schema.nodes.contains_key(&r) && &r != name {
                        graph.add_edge(&r, name);
                    }
                }
            }
        }

        graph
    }

    fn add_edge(&mut self, from: &str, to: &str) {
        if let Some(deps) = self.dependencies.get_mut(to) {
            deps.insert(from.to_string());
        }
    }

    pub fn dependencies_of(&self, node: &str) -> Option<&HashSet<String>> {
        self.dependencies.get(node)
    }
}

fn extract_references(value: &serde_json::Value) -> HashSet<String> {
    let mut refs = HashSet::new();
    match value {
        serde_json::Value::String(s) => {
            for r in extract_from_string(s) {
                refs.insert(r);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                refs.extend(extract_references(v));
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values() {
                refs.extend(extract_references(v));
            }
        }
        _ => {}
    }
    refs
}

fn extract_from_string(s: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut start = 0;
    while let Some(open) = s[start..].find("{{") {
        let open_idx = start + open;
        if let Some(close) = s[open_idx + 2..].find("}}") {
            let close_idx = open_idx + 2 + close;
            let inner = s[open_idx + 2..close_idx].trim();
            if let Some(dot_idx) = inner.find('.') {
                refs.push(inner[..dot_idx].to_string());
            } else {
                refs.push(inner.to_string());
            }
            start = close_idx + 2;
        } else {
            break;
        }
    }
    refs
}
