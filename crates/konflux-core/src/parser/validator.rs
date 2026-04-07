//! Workflow validation — enforces all strictness rules.

use std::collections::HashSet;

use crate::error::ValidationError;
use crate::parser::schema::{WorkflowSchema, NodeSchema, DoBlock, ThenBlock, PipeStepSchema};

/// Validate a parsed workflow schema.
pub fn validate(schema: &WorkflowSchema) -> Result<(), ValidationError> {
    let mut validator = Validator::new(schema);
    validator.validate()
}

struct Validator<'a> {
    schema: &'a WorkflowSchema,
    node_names: HashSet<String>,
}

impl<'a> Validator<'a> {
    fn new(schema: &'a WorkflowSchema) -> Self {
        let node_names = schema.nodes.keys().cloned().collect();
        Self {
            schema,
            node_names,
        }
    }

    fn validate(&mut self) -> Result<(), ValidationError> {
        if self.schema.nodes.is_empty() {
            return Err(ValidationError::NoEntryNode);
        }

        // 1. Check if entry node is valid
        let entry_id = self.schema.nodes.keys().next().unwrap();
        
        // 2. Validate each node
        for (name, node) in &self.schema.nodes {
            self.validate_node(name, node)?;
        }

        // 3. Check reachability and cycles
        self.check_reachability_and_cycles(entry_id)?;

        // 4. Check for at least one return node
        self.check_terminals()?;

        Ok(())
    }

    fn validate_node(&self, name: &str, node: &NodeSchema) -> Result<(), ValidationError> {
        // Validate tool references and capabilities
        if let Some(do_block) = &node.do_ {
            match do_block {
                DoBlock::Single(tool) => self.check_tool(tool, name)?,
                DoBlock::Parallel(tasks) => {
                    for task_map in tasks {
                        for task in task_map.values() {
                            self.check_tool(&task.tool, name)?;
                        }
                    }
                }
            }
        }

        // Validate pipe steps
        for pipe_step in &node.pipe {
            match pipe_step {
                PipeStepSchema::Simple(tool) => self.check_tool(tool, name)?,
                PipeStepSchema::WithArgs(map) => {
                    if let Some(tool) = map.keys().next() {
                        self.check_tool(tool, name)?;
                    }
                }
                PipeStepSchema::Full { do_, .. } => self.check_tool(do_, name)?,
            }
        }

        // Validate then block targets
        if let Some(then_block) = &node.then {
            match then_block {
                ThenBlock::Unconditional(target) => {
                    if !self.node_names.contains(target) {
                        return Err(ValidationError::UnknownNode {
                            node: target.clone(),
                            from: name.to_string(),
                        });
                    }
                }
                ThenBlock::Multiple(targets) => {
                    for target in targets {
                        if !self.node_names.contains(target) {
                            return Err(ValidationError::UnknownNode {
                                node: target.clone(),
                                from: name.to_string(),
                            });
                        }
                    }
                }
                ThenBlock::Conditional(branches) => {
                    for branch in branches {
                        let target = branch.then.as_ref().or(branch.goto.as_ref());
                        if let Some(t) = target.filter(|t| !self.node_names.contains(*t)) {
                            return Err(ValidationError::UnknownNode {
                                node: t.clone(),
                                from: name.to_string(),
                            });
                        }
                    }
                }
            }
        }

        // Validate catch targets
        for branch in &node.catch {
            if let Some(target) = branch.then.as_ref().filter(|t| !self.node_names.contains(*t)) {
                return Err(ValidationError::UnknownNode {
                    node: target.clone(),
                    from: name.to_string(),
                });
            }
            if let Some(do_) = &branch.do_ {
                // do_ in catch can be a tool or "skip"
                if do_ != "skip" && do_ != "continue" && !do_.starts_with("fallback:") {
                    self.check_tool(do_, name)?;
                }
            }
        }

        Ok(())
    }

    fn check_tool(&self, tool: &str, _node_name: &str) -> Result<(), ValidationError> {
        // If capabilities are declared, tool must be allowed by them
        if !self.schema.capabilities.is_empty() {
            let allowed = self.schema.capabilities.iter().any(|cap| {
                if cap == "*" {
                    true
                } else if let Some(prefix) = cap.strip_suffix(":*") {
                    tool.starts_with(prefix) && tool.get(prefix.len()..prefix.len()+1) == Some(":")
                } else {
                    cap == tool
                }
            });

            if !allowed {
                return Err(ValidationError::MissingCapability {
                    capability: tool.to_string(),
                });
            }
        }
        Ok(())
    }

    fn check_terminals(&self) -> Result<(), ValidationError> {
        let has_terminal = self.schema.nodes.values().any(|n| n.return_.is_some());
        if !has_terminal {
            return Err(ValidationError::NoReturnNode);
        }
        Ok(())
    }

    fn check_reachability_and_cycles(&self, entry_id: &str) -> Result<(), ValidationError> {
        let mut visited = HashSet::new();
        let mut path = Vec::new();
        let mut reachable = HashSet::new();

        self.dfs(entry_id, &mut visited, &mut path, &mut reachable)?;

        for node_name in &self.node_names {
            if !reachable.contains(node_name) {
                return Err(ValidationError::OrphanedNode {
                    node: node_name.clone(),
                });
            }
        }

        Ok(())
    }

    fn dfs(
        &self,
        current: &str,
        visited: &mut HashSet<String>,
        path: &mut Vec<String>,
        reachable: &mut HashSet<String>,
    ) -> Result<(), ValidationError> {
        if path.contains(&current.to_string()) {
            let mut cycle = path.clone();
            cycle.push(current.to_string());
            return Err(ValidationError::CycleDetected { path: cycle });
        }

        reachable.insert(current.to_string());

        if visited.contains(current) {
            return Ok(());
        }

        visited.insert(current.to_string());
        path.push(current.to_string());

        if let Some(node) = self.schema.nodes.get(current) {
            // Successors from then:
            if let Some(then_block) = &node.then {
                match then_block {
                    ThenBlock::Unconditional(target) => {
                        self.dfs(target, visited, path, reachable)?;
                    }
                    ThenBlock::Multiple(targets) => {
                        for target in targets {
                            self.dfs(target, visited, path, reachable)?;
                        }
                    }
                    ThenBlock::Conditional(branches) => {
                        for branch in branches {
                            let target = branch.then.as_ref().or(branch.goto.as_ref());
                            if let Some(t) = target {
                                self.dfs(t, visited, path, reachable)?;
                            }
                        }
                    }
                }
            }

            // Successors from catch:
            for branch in &node.catch {
                if let Some(target) = &branch.then {
                    self.dfs(target, visited, path, reachable)?;
                }
            }
        }

        path.pop();
        Ok(())
    }
}
