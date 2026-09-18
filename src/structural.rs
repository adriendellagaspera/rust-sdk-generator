use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::contracts::Bindings;
use crate::rust_type::{Type, parse_type};

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
    pub option_depth: usize,
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

fn nullable_schema(schema: &Value) -> Option<&Value> {
    let branches = schema.get("anyOf").and_then(Value::as_array)?;
    if branches.len() != 2 {
        return None;
    }
    let mut non_null = None;
    let mut nulls = 0;
    for branch in branches {
        if branch.get("type").and_then(Value::as_str) == Some("null") {
            nulls += 1;
        } else if non_null.is_none() {
            non_null = Some(branch);
        } else {
            return None;
        }
    }
    (nulls == 1).then_some(non_null?)
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

fn rust_scalar(type_name: &str) -> Option<(ScalarKind, usize)> {
    let syntax = parse_type(type_name).ok()?;
    let (inner, option_depth) = if let Some(inner) = syntax.unary("Option") {
        if let Some(nullable) = inner.unary("Option") {
            if nullable.unary("Option").is_some() {
                return None;
            }
            (nullable.spelling.as_str(), 2)
        } else {
            (inner.spelling.as_str(), 1)
        }
    } else {
        (syntax.spelling.as_str(), 0)
    };
    let kind = match inner {
        "String" => ScalarKind::String,
        "bool" => ScalarKind::Boolean,
        value if integer_rust_type(value) => ScalarKind::Integer,
        "f32" | "f64" => ScalarKind::Number,
        _ => return None,
    };
    Some((kind, option_depth))
}

fn type_matches_schema(
    schema: &Value,
    syntax: &Type,
    bindings: &Bindings,
    seen_aliases: &mut BTreeSet<String>,
) -> bool {
    if let Some(alias) = bindings.aliases.get(&syntax.spelling) {
        if !seen_aliases.insert(syntax.spelling.clone()) {
            return false;
        }
        let matched = parse_type(alias)
            .ok()
            .is_some_and(|expanded| type_matches_schema(schema, &expanded, bindings, seen_aliases));
        seen_aliases.remove(&syntax.spelling);
        return matched;
    }

    if let Some(non_null) = nullable_schema(schema) {
        return syntax
            .unary("Option")
            .is_some_and(|inner| type_matches_schema(non_null, inner, bindings, seen_aliases));
    }
    if syntax.unary("Option").is_some() {
        return false;
    }

    match schema.get("type").and_then(Value::as_str) {
        Some("string") => syntax.spelling == "String",
        Some("boolean") => syntax.spelling == "bool",
        Some("integer") => integer_rust_type(&syntax.spelling),
        Some("number") => matches!(syntax.spelling.as_str(), "f32" | "f64"),
        Some("array") => schema.get("items").is_some_and(|items| {
            syntax
                .unary("Vec")
                .is_some_and(|inner| type_matches_schema(items, inner, bindings, seen_aliases))
        }),
        Some("object") => {
            if let Some(wire) = scalar_object_shape(schema) {
                raw_scalar_struct_shape(bindings, &syntax.spelling).is_some_and(|raw| raw == wire)
            } else {
                schema
                    .get("additionalProperties")
                    .filter(|additional| **additional != Value::Bool(false))
                    .is_some_and(|additional| {
                        syntax.constructor.as_deref() == Some("std::collections::BTreeMap")
                            && syntax.arguments.len() == 2
                            && syntax.arguments[0].spelling == "String"
                            && type_matches_schema(
                                additional,
                                &syntax.arguments[1],
                                bindings,
                                seen_aliases,
                            )
                    })
            }
        }
        _ => false,
    }
}

pub(crate) fn rust_type_matches_schema(
    schema: &Value,
    type_name: &str,
    bindings: &Bindings,
) -> bool {
    parse_type(type_name)
        .ok()
        .is_some_and(|syntax| type_matches_schema(schema, &syntax, bindings, &mut BTreeSet::new()))
}

fn expand_alias_syntax(
    mut syntax: Type,
    bindings: &Bindings,
    seen_aliases: &mut BTreeSet<String>,
) -> Option<Type> {
    while let Some(alias) = bindings.aliases.get(&syntax.spelling) {
        if !seen_aliases.insert(syntax.spelling.clone()) {
            return None;
        }
        syntax = parse_type(alias).ok()?;
    }
    Some(syntax)
}

pub(crate) fn inline_array_object_item(
    schema: &Value,
    raw: &str,
    bindings: &Bindings,
) -> Option<String> {
    if schema.get("type").and_then(Value::as_str) != Some("array") {
        return None;
    }
    let items = schema.get("items")?;
    let wire = scalar_object_shape(items)?;
    let root = expand_alias_syntax(parse_type(raw).ok()?, bindings, &mut BTreeSet::new())?;
    let inner = root.unary("Vec")?;
    let item = expand_alias_syntax(inner.clone(), bindings, &mut BTreeSet::new())?;
    raw_scalar_struct_shape(bindings, &item.spelling)
        .filter(|raw_shape| *raw_shape == wire)
        .map(|_| item.spelling)
}

pub(crate) fn scalar_object_shape(schema: &Value) -> Option<BTreeMap<String, ScalarFieldShape>> {
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
    let required: BTreeSet<_> = required_values.iter().filter_map(Value::as_str).collect();
    if required.len() != required_values.len()
        || required
            .iter()
            .any(|field| !properties.contains_key(*field))
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
                option_depth: usize::from(!required.contains(name.as_str()))
                    + usize::from(nullable),
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
        let (kind, option_depth) = rust_scalar(&field.type_name)?;
        if result
            .insert(name.to_owned(), ScalarFieldShape { kind, option_depth })
            .is_some()
        {
            return None;
        }
    }
    Some(result)
}

pub(crate) fn inline_object_union_mapping(
    schema: &Value,
    raw_union: &str,
    bindings: &Bindings,
) -> Option<Vec<(String, String)>> {
    let branches = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)?;
    if branches.len() < 2 {
        return None;
    }

    let wire_shapes = branches
        .iter()
        .map(scalar_object_shape)
        .collect::<Option<Vec<_>>>()?;
    let variants = bindings.enums.get(raw_union)?;
    if variants.len() != wire_shapes.len()
        || variants.iter().any(|variant| variant.payload.is_none())
    {
        return None;
    }

    let raw_shapes = variants
        .iter()
        .map(|variant| {
            let payload = variant.payload.as_ref()?;
            Some((
                variant.name.clone(),
                payload.clone(),
                raw_scalar_struct_shape(bindings, payload)?,
            ))
        })
        .collect::<Option<Vec<_>>>()?;

    let branch_matches = wire_shapes
        .iter()
        .map(|wire| {
            raw_shapes
                .iter()
                .enumerate()
                .filter(|(_, (_, _, raw))| raw == wire)
                .map(|(index, _)| index)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    if branch_matches.iter().any(|matches| matches.len() != 1) {
        return None;
    }

    for raw_index in 0..raw_shapes.len() {
        if branch_matches
            .iter()
            .filter(|matches| matches[0] == raw_index)
            .count()
            != 1
        {
            return None;
        }
    }

    Some(
        branch_matches
            .into_iter()
            .map(|matches| {
                let (variant, payload, _) = &raw_shapes[matches[0]];
                (variant.clone(), payload.clone())
            })
            .collect(),
    )
}
