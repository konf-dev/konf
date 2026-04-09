//! Compiler: transforms parsed YAML into executor IR.

use std::collections::HashMap;
use std::time::Duration;

use crate::error::ParseError;
use crate::parser::graph::DependencyGraph;
use crate::parser::schema::{
    CatchBlock, DoBlock, NodeSchema, PipeStepSchema, ThenBlock, WorkflowSchema,
};
use crate::workflow::{
    BackoffStrategy, Edge, EdgeTarget, ErrorAction, Expr, PipeStep, RepeatConfig, RetryPolicy,
    Step, StepId, ToolId, Workflow,
};

/// Compile a validated WorkflowSchema into a Workflow IR.
pub fn compile(schema: WorkflowSchema, graph: &DependencyGraph) -> Result<Workflow, ParseError> {
    let mut steps = Vec::new();

    // Determine entry point: first node in YAML order
    let entry_id = schema
        .nodes
        .keys()
        .next()
        .cloned()
        .ok_or_else(|| ParseError::InvalidYaml {
            message: "Workflow must have at least one node".to_string(),
        })?;

    for (name, node) in &schema.nodes {
        let mut step = compile_node(name, node, &schema)?;
        if let Some(deps) = graph.dependencies_of(name) {
            step.depends_on = deps.iter().map(StepId::new).collect();
        }
        steps.push(step);
    }

    let mut workflow = Workflow::new(schema.workflow.clone(), schema.workflow.clone(), entry_id);
    workflow.version = schema.version;
    workflow.steps = steps;
    workflow.capabilities = schema.capabilities;
    workflow.description = schema.description;
    workflow.input_schema = schema.input_schema;
    workflow.output_schema = schema.output_schema;
    workflow.register_as_tool = schema.register_as_tool;

    Ok(workflow)
}

fn compile_node(
    name: &str,
    node: &NodeSchema,
    _schema: &WorkflowSchema,
) -> Result<Step, ParseError> {
    let tool_id = match &node.do_ {
        Some(DoBlock::Single(tool)) => ToolId::new(tool),
        Some(DoBlock::Parallel(_)) => ToolId::new(format!("internal_parallel_{}", name)),
        None => ToolId::new("echo"), // default to echo for pass-through
    };

    let mut step = Step::new(name, tool_id.as_str());

    // Compile input expressions
    if let Some(serde_json::Value::Object(map)) = &node.with {
        for (key, value) in map {
            step = step.with_input(key, value_to_expr(value));
        }
    } else if node.do_.is_none() {
        if let Some(ret) = &node.return_ {
            step = step.with_input("__return__", value_to_expr(ret));
        }
    }

    // Compile edges
    if node.return_.is_some() {
        step.edges.push(Edge {
            target: EdgeTarget::Return,
            condition: None,
            priority: 0,
        });
    }

    match &node.then {
        Some(ThenBlock::Unconditional(target)) => {
            step.edges.push(Edge {
                target: EdgeTarget::Step(StepId::new(target)),
                condition: None,
                priority: 0,
            });
        }
        Some(ThenBlock::Multiple(targets)) => {
            for target in targets {
                step.edges.push(Edge {
                    target: EdgeTarget::Step(StepId::new(target)),
                    condition: None,
                    priority: 0,
                });
            }
        }
        Some(ThenBlock::Conditional(branches)) => {
            for (i, branch) in branches.iter().enumerate() {
                let condition = if branch.else_.unwrap_or(false) {
                    None
                } else {
                    branch.when.clone()
                };

                let target = branch.then.as_ref().or(branch.goto.as_ref());
                if let Some(target_node) = target {
                    step.edges.push(Edge {
                        target: EdgeTarget::Step(StepId::new(target_node)),
                        condition,
                        priority: i as i32,
                    });
                }
            }
        }
        None => {}
    }

    // Compile error handling
    match &node.catch {
        CatchBlock::Simple(target) => {
            step.on_error = ErrorAction::Goto {
                step: StepId::new(target),
            };
        }
        CatchBlock::Branches(branches) if !branches.is_empty() => {
            for branch in branches {
                let is_match = branch.is_match();

                if is_match {
                    if let Some(target) = &branch.then {
                        step.on_error = ErrorAction::Goto {
                            step: StepId::new(target),
                        };
                    } else if let Some(do_) = &branch.do_ {
                        if do_ == "skip" || do_ == "continue" {
                            step.on_error = ErrorAction::Skip;
                        } else if do_.starts_with("fallback:") {
                            let value = do_.strip_prefix("fallback:").unwrap_or("").to_string();
                            step.on_error = ErrorAction::Fallback { value };
                        }
                    }
                    break;
                }
            }
        }
        _ => {} // No catch configured
    }

    // Compile retry policy
    if let Some(retry) = &node.retry {
        let base_delay = match retry.delay.as_ref() {
            Some(d) => parse_duration(d)?,
            None => Duration::from_secs(1),
        };
        step.retry = Some(RetryPolicy {
            max_attempts: retry.times,
            backoff: match retry.backoff.as_deref() {
                Some("fixed") => BackoffStrategy::Fixed,
                Some("linear") => BackoffStrategy::Linear {
                    increment: base_delay,
                },
                _ => BackoffStrategy::Exponential,
            },
            base_delay,
            max_delay: match retry.max_delay.as_ref() {
                Some(d) => parse_duration(d)?,
                None => Duration::from_secs(30),
            },
        });
    }

    // Compile timeout
    step.timeout = match node.timeout.as_ref() {
        Some(t) => Some(parse_duration(t)?),
        None => None,
    };

    // Compile credentials
    step.credentials = node.credentials.clone();

    // Compile grant
    step.grant = node.grant.clone();

    // Compile pipe
    for pipe_step in &node.pipe {
        match pipe_step {
            PipeStepSchema::Simple(tool) => {
                step.pipe.push(PipeStep {
                    tool: ToolId::new(tool),
                    input: HashMap::new(),
                });
            }
            PipeStepSchema::WithArgs(map) => {
                if let Some((tool, args)) = map.iter().next() {
                    let mut input = HashMap::new();
                    if let serde_json::Value::Object(obj) = args {
                        for (k, v) in obj {
                            input.insert(k.clone(), value_to_expr(v));
                        }
                    }
                    step.pipe.push(PipeStep {
                        tool: ToolId::new(tool),
                        input,
                    });
                }
            }
            PipeStepSchema::Full { do_, with } => {
                let mut input = HashMap::new();
                if let Some(serde_json::Value::Object(obj)) = with {
                    for (k, v) in obj {
                        input.insert(k.clone(), value_to_expr(v));
                    }
                }
                step.pipe.push(PipeStep {
                    tool: ToolId::new(do_),
                    input,
                });
            }
        }
    }

    // Compile stream
    step.stream = node.stream.to_mode();

    // Compile repeat
    if let Some(repeat) = &node.repeat {
        step.repeat = Some(RepeatConfig {
            until: repeat.until.clone(),
            max: repeat.max,
            as_var: repeat.as_.clone(),
        });
    }

    Ok(step)
}

fn value_to_expr(value: &serde_json::Value) -> Expr {
    match value {
        serde_json::Value::String(s) => {
            if s.starts_with("{{") && s.ends_with("}}") && !s[2..s.len() - 2].contains("{{") {
                let inner = &s[2..s.len() - 2].trim();
                if inner
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '.' || c == '_')
                {
                    Expr::Ref(inner.to_string())
                } else {
                    Expr::Template(s.clone())
                }
            } else if s.contains("{{") && s.contains("}}") {
                Expr::Template(s.clone())
            } else {
                Expr::Literal(s.clone())
            }
        }
        _ => Expr::Json(value.clone()),
    }
}

fn parse_duration(s: &str) -> Result<Duration, ParseError> {
    let s = s.trim();
    let err = |detail: &str| ParseError::InvalidValue {
        field: "duration".into(),
        message: format!("{detail} (expected Ns, Nms, or Nm, got '{s}')"),
    };
    if let Some(ms) = s.strip_suffix("ms") {
        let n = ms
            .trim()
            .parse::<u64>()
            .map_err(|_| err("invalid milliseconds"))?;
        Ok(Duration::from_millis(n))
    } else if let Some(secs) = s.strip_suffix('s') {
        let n = secs
            .trim()
            .parse::<u64>()
            .map_err(|_| err("invalid seconds"))?;
        Ok(Duration::from_secs(n))
    } else if let Some(mins) = s.strip_suffix('m') {
        let n = mins
            .trim()
            .parse::<u64>()
            .map_err(|_| err("invalid minutes"))?;
        Ok(Duration::from_secs(n * 60))
    } else {
        let n = s
            .parse::<u64>()
            .map_err(|_| err("invalid duration format"))?;
        Ok(Duration::from_secs(n))
    }
}
