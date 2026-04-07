//! Expression Evaluation Engine
//!
//! Evaluates condition expressions against workflow state.
//! Supports a safe subset of expressions (no arbitrary code execution).

use std::collections::HashMap;
use serde_json::Value;
use tracing::{debug};

/// The result of evaluating an expression
#[derive(Debug, Clone, PartialEq)]
pub enum ExprValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Vec<ExprValue>),
    Map(HashMap<String, ExprValue>),
    Json(Value),
    Null,
}

/// Errors that can occur during expression evaluation
#[derive(Debug, Clone, thiserror::Error)]
pub enum ExprError {
    #[error("parse error: {0}")]
    ParseError(String),
    #[error("type error: expected {expected}, got {got}")]
    TypeError { expected: String, got: String },
    #[error("unknown reference: {0}")]
    UnknownReference(String),
    #[error("division by zero")]
    #[allow(dead_code)]
    DivisionByZero,
}

impl From<Value> for ExprValue {
    fn from(value: Value) -> Self {
        ExprEvaluator::json_to_expr_value(&value)
    }
}

/// A simple expression evaluator
pub struct ExprEvaluator<'a> {
    context: &'a HashMap<String, ExprValue>,
}

impl<'a> ExprEvaluator<'a> {
    pub fn new(context: &'a HashMap<String, ExprValue>) -> Self {
        Self { context }
    }

    pub fn evaluate(&self, expr: &str) -> Result<ExprValue, ExprError> {
        let expr = expr.trim();
        if expr.is_empty() {
            return Ok(ExprValue::Bool(true));
        }
        Self::eval_expr(expr, self.context)
    }

    pub fn evaluate_as_bool(&self, expr: &str) -> Result<bool, ExprError> {
        let res = self.evaluate(expr)?;
        let b = Self::to_bool(&res);
        debug!("Evaluated '{}' to {}", expr, b);
        Ok(b)
    }
    
    fn eval_expr(expr: &str, context: &HashMap<String, ExprValue>) -> Result<ExprValue, ExprError> {
        let expr = expr.trim();
        
        // 1. Logical OR (lowest precedence)
        if let Some(idx) = expr.rfind(" || ") {
            let left = Self::eval_expr(&expr[..idx], context)?;
            let right = Self::eval_expr(&expr[idx + 4..], context)?;
            return Ok(ExprValue::Bool(Self::to_bool(&left) || Self::to_bool(&right)));
        }

        // 2. Logical AND
        if let Some(idx) = expr.rfind(" && ") {
            let left = Self::eval_expr(&expr[..idx], context)?;
            let right = Self::eval_expr(&expr[idx + 4..], context)?;
            return Ok(ExprValue::Bool(Self::to_bool(&left) && Self::to_bool(&right)));
        }

        // 3. Comparisons
        let ops = [(" == ", "=="), (" != ", "!="), (" >= ", ">="), (" <= ", "<="), (" > ", ">"), (" < ", "<")];
        for (sep, op) in ops {
            if let Some(idx) = expr.find(sep) {
                let left_str = expr[..idx].trim();
                let right_str = expr[idx + sep.len()..].trim();
                
                let left = Self::eval_expr(left_str, context)?;
                let right = Self::eval_expr(right_str, context)?;
                
                let result = match op {
                    "==" => ExprValue::Bool(Self::equals(&left, &right)),
                    "!=" => ExprValue::Bool(!Self::equals(&left, &right)),
                    ">" => ExprValue::Bool(Self::compare(&left, &right)? > 0),
                    "<" => ExprValue::Bool(Self::compare(&left, &right)? < 0),
                    ">=" => ExprValue::Bool(Self::compare(&left, &right)? >= 0),
                    "<=" => ExprValue::Bool(Self::compare(&left, &right)? <= 0),
                    _ => unreachable!(),
                };
                debug!("Comparison result: {:?} {} {:?} -> {:?}", left, op, right, result);
                return Ok(result);
            }
        }

        // 4. Unary NOT
        if let Some(stripped) = expr.strip_prefix('!') {
            let val = Self::eval_expr(stripped, context)?;
            return Ok(ExprValue::Bool(!Self::to_bool(&val)));
        }

        // 5. Keywords
        if let Some(res) = Self::try_keyword_op(expr, context)? {
            return Ok(res);
        }

        // 6. Basic Values
        Self::eval_value(expr, context)
    }

    fn try_keyword_op(expr: &str, context: &HashMap<String, ExprValue>) -> Result<Option<ExprValue>, ExprError> {
        if let Some(stripped) = expr.strip_suffix(" exists") {
            let reference = stripped.trim();
            return match Self::eval_value(reference, context) {
                Ok(val) => Ok(Some(ExprValue::Bool(!matches!(val, ExprValue::Null)))),
                Err(_) => Ok(Some(ExprValue::Bool(false))),
            };
        }
        
        if let Some(stripped) = expr.strip_suffix(" is empty") {
            let reference = stripped.trim();
            let val = Self::eval_value(reference, context)?;
            let is_empty = match val {
                ExprValue::String(s) => s.is_empty(),
                ExprValue::List(l) => l.is_empty(),
                ExprValue::Map(m) => m.is_empty(),
                ExprValue::Json(Value::Array(a)) => a.is_empty(),
                ExprValue::Json(Value::Object(o)) => o.is_empty(),
                ExprValue::Json(Value::String(s)) => s.is_empty(),
                ExprValue::Null => true,
                _ => false,
            };
            return Ok(Some(ExprValue::Bool(is_empty)));
        }
        Ok(None)
    }
    
    fn eval_value(expr: &str, context: &HashMap<String, ExprValue>) -> Result<ExprValue, ExprError> {
        let expr = expr.trim();
        if expr == "true" { return Ok(ExprValue::Bool(true)); }
        if expr == "false" { return Ok(ExprValue::Bool(false)); }
        if expr == "null" || expr == "None" { return Ok(ExprValue::Null); }
        
        if expr.len() >= 2
            && ((expr.starts_with('"') && expr.ends_with('"'))
                || (expr.starts_with('\'') && expr.ends_with('\'')))
        {
            return Ok(ExprValue::String(expr[1..expr.len()-1].to_string()));
        }
        
        if let Ok(n) = expr.parse::<i64>() { return Ok(ExprValue::Int(n)); }
        if let Ok(f) = expr.parse::<f64>() { return Ok(ExprValue::Float(f)); }
        
        let parts: Vec<&str> = expr.split('.').collect();
        if let Some(mut current) = context.get(parts[0]).cloned() {
            for part in &parts[1..] {
                match current {
                    ExprValue::Map(ref m) => {
                        if let Some(v) = m.get(*part) {
                            current = v.clone();
                        } else {
                            return Err(ExprError::UnknownReference(format!("{}: field '{}' not found", expr, part)));
                        }
                    }
                    ExprValue::Json(ref j) => {
                        if let Some(v) = j.get(*part) {
                            current = Self::json_to_expr_value(v);
                        } else {
                            return Err(ExprError::UnknownReference(format!("{}: field '{}' not found in JSON", expr, part)));
                        }
                    }
                    _ => return Err(ExprError::TypeError { expected: "map or object".into(), got: format!("{:?}", current) }),
                }
            }
            return Ok(current);
        }
        Err(ExprError::UnknownReference(expr.to_string()))
    }
    
    pub fn json_to_expr_value(json: &Value) -> ExprValue {
        match json {
            Value::Null => ExprValue::Null,
            Value::Bool(b) => ExprValue::Bool(*b),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() { ExprValue::Int(i) }
                else if let Some(f) = n.as_f64() { ExprValue::Float(f) }
                else { ExprValue::String(n.to_string()) }
            }
            Value::String(s) => ExprValue::String(s.clone()),
            Value::Array(arr) => ExprValue::List(arr.iter().map(Self::json_to_expr_value).collect()),
            Value::Object(obj) => {
                let mut map = HashMap::new();
                for (k, v) in obj { map.insert(k.clone(), Self::json_to_expr_value(v)); }
                ExprValue::Map(map)
            }
        }
    }
    
    fn to_bool(val: &ExprValue) -> bool {
        match val {
            ExprValue::Bool(b) => *b,
            ExprValue::Int(n) => *n != 0,
            ExprValue::Float(f) => *f != 0.0,
            ExprValue::String(s) => !s.is_empty(),
            ExprValue::List(l) => !l.is_empty(),
            ExprValue::Map(m) => !m.is_empty(),
            ExprValue::Json(j) => {
                if let Some(b) = j.as_bool() { b }
                else if let Some(s) = j.as_str() { !s.is_empty() }
                else { !j.is_null() }
            }
            ExprValue::Null => false,
        }
    }
    
    fn equals(a: &ExprValue, b: &ExprValue) -> bool {
        match (a, b) {
            (ExprValue::Bool(a), ExprValue::Bool(b)) => a == b,
            (ExprValue::Int(a), ExprValue::Int(b)) => a == b,
            (ExprValue::Float(a), ExprValue::Float(b)) => (a - b).abs() < f64::EPSILON,
            (ExprValue::String(a), ExprValue::String(b)) => a == b,
            (ExprValue::Json(Value::String(s)), ExprValue::String(b)) => s == b,
            (ExprValue::String(a), ExprValue::Json(Value::String(s))) => a == s,
            (ExprValue::Json(Value::Number(n)), ExprValue::Int(b)) => n.as_i64() == Some(*b),
            (ExprValue::Int(a), ExprValue::Json(Value::Number(n))) => Some(*a) == n.as_i64(),
            (ExprValue::Json(Value::Number(n)), ExprValue::Float(b)) => n.as_f64() == Some(*b),
            (ExprValue::Float(a), ExprValue::Json(Value::Number(n))) => Some(*a) == n.as_f64(),
            (ExprValue::Json(a), ExprValue::Json(b)) => a == b,
            (ExprValue::Null, ExprValue::Null) => true,
            _ => {
                if let (ExprValue::Json(ja), ExprValue::Json(jb)) = (a, b) {
                    return ja == jb;
                }
                false
            }
        }
    }
    
    fn compare(a: &ExprValue, b: &ExprValue) -> Result<i32, ExprError> {
        let va = Self::to_float(a)?;
        let vb = Self::to_float(b)?;
        Ok(if va < vb { -1 } else if va > vb { 1 } else { 0 })
    }

    fn to_float(val: &ExprValue) -> Result<f64, ExprError> {
        match val {
            ExprValue::Int(i) => Ok(*i as f64),
            ExprValue::Float(f) => Ok(*f),
            ExprValue::String(s) => s.parse::<f64>().map_err(|e| ExprError::TypeError {
                expected: "numeric string".into(),
                got: format!("'{}' error: {}", s, e)
            }),
            ExprValue::Json(Value::Number(n)) => n.as_f64().ok_or(ExprError::TypeError { 
                expected: "number".into(), 
                got: format!("json non-number: {:?}", n) 
            }),
            ExprValue::Json(Value::String(s)) => s.parse::<f64>().map_err(|e| ExprError::TypeError {
                expected: "numeric string".into(),
                got: format!("'{}' error: {}", s, e)
            }),
            _ => Err(ExprError::TypeError { expected: "number".into(), got: format!("{:?}", val) }),
        }
    }
}
