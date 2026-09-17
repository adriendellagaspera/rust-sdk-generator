use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::contracts::Bindings;
use crate::rust_type::parse_type;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScalarKind {
    String,
    Boolean,
    Integer,
    Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScalarFieldShape {
    pub kind: ScalarKind,
    pub optional: bool,
}

fn direct_scalar_schema(schema: &Value) -> Option<ScalarKind> {
    match schema.get("type").and_then(Value::as_str)? {
        "string" => Some(ScalarKind::String),
        "boolean" => Some(ScalarKind::Boolean),
        "integer" => Some(ScalarKind::Integer),
        "number" => Some(ScalarKind::Number),
        _ => None,
    }
}

fn scalar_schema(schema: &Value) -> Option<(ScalarKind, bool)> {
    if let Some(kind) = direct_scalar_schema(schema) {
        return Some((kind, false));
    }
    let branches = schema.get("anyOf").and_then(Value::as_array)?;
    if branches.len() != 2 {
        return None;
    }
    let mut scalar = None;
    let mut nulls = 0;
    for branch in branches {
        if branch.get("type").and_then(Value::as_str) == Some("null") {
            nulls += 1;
        } else if scalar.is_none() {
            scalar = direct_scalar_schema(branch);
        } else {
            return None;
        }
    }
    (nulls == 1).then_some((scalar?, true))
}

fn integer_rust_type(value: &str) -> bool {
    matches!(
        value,
        "i8" | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
    )
}

fn rust_scalar(type_name: &str) -> Option<(ScalarKind, bool)> {
    let syntax = parse_type(type_name).ok()?;
    let (inner, optional) = if let Some(inner) = syntax.unary("Option") {
        if inner.unary("Option").is_some() {
            return None;
        }
        (inner.spelling.as_str(), true)
    } else {
        (syntax.spelling.as_str(), false)
    };
    let kind = match inner {
        "String" => ScalarKind::String,
        "bool" => ScalarKind::Boolean,
        value if integer_rust_type(value) => ScalarKind::Integer,
        "f32" | "f64" => ScalarKind::Number,
        _ => return None,
    };
    Some((kind, optional))
}

pub(crate) fn scalar_object_shape(
    schema: &Value,
) -> Option<BTreeMap<String, ScalarFieldShape>> {
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return None;
    }
    if schema
        .get("additionalProperties")
        .is_some_and(|additional| additional != &Value::Bool(false))
    {
        return None;
    }
    let properties = schema.get("properties").and_then(Value::as_object)?;
    let required_values = schema
        .get("required")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let required: BTreeSet<_> = required_values
        .iter()
        .filter_map(Value::as_str)
        .collect();
    if required.len() != required_values.len()
        || required.iter().any(|field| !properties.contains_key(*field))
    {
        return None;
    }

    let mut result = BTreeMap::new();
    for (name, property) in properties {
        let (kind, nullable) = scalar_schema(property)?;
        result.insert(
            name.clone(),
            ScalarFieldShape {
                kind,
                optional: !required.contains(name.as_str()) || nullable,
            },
        );
    }
    Some(result)
}

pub(crate) fn raw_scalar_struct_shape(
    bindings: &Bindings,
    raw: &str,
) -> Option<BTreeMap<String, ScalarFieldShape>> {
    let fields = bindings.structs.get(raw)?;
    let mut result = BTreeMap::new();
    for field in fields {
        let name = field.name.strip_prefix("r#").unwrap_or(&field.name);
        let (kind, optional) = rust_scalar(&field.type_name)?;
        if result
            .insert(name.to_owned(), ScalarFieldShape { kind, optional })
            .is_some()
        {
            return None;
        }
    }
    Some(result)
}
