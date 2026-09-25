use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::contracts::{
    Bindings, OperationBinding, OperationBindingKind, RequestDiscriminatorBinding,
    RequestDiscriminatorValue,
};
use crate::openapi::{OpenApiIndex, ref_name};
use crate::rust_type::{Type, TypeKind, parse_type};

pub(crate) fn multipart_filenames_binding<'a>(
    bindings: &'a Bindings,
    primary_name: &str,
) -> std::result::Result<Option<(&'a str, &'a OperationBinding)>, &'static str> {
    let primary = bindings
        .operations
        .get(primary_name)
        .ok_or("bindings.no_structural_match")?;
    let Some(primary_metadata) = &primary.metadata else {
        return Ok(None);
    };
    if primary_metadata.kind != OperationBindingKind::CallShape {
        return Err("bindings.multipart_filenames_identity_mismatch");
    }

    let source_helpers: Vec<_> = bindings
        .operations
        .iter()
        .filter(|(_, candidate)| {
            candidate.metadata.as_ref().is_some_and(|metadata| {
                metadata.kind == OperationBindingKind::MultipartFilenames
                    && metadata.source_operation == primary_metadata.source_operation
            })
        })
        .collect();
    if source_helpers.is_empty() {
        return Ok(None);
    }

    let primary_parameters: BTreeSet<_> = primary
        .parameters
        .iter()
        .map(|parameter| (parameter.name.as_str(), parameter.type_name.as_str()))
        .collect();
    if primary_parameters.len() != primary.parameters.len()
        || primary_parameters
            .iter()
            .any(|(name, _)| *name == "multipart_filenames")
    {
        return Err("bindings.multipart_filenames_signature_mismatch");
    }

    let matching: Vec<_> = source_helpers
        .into_iter()
        .filter(|(_, helper)| {
            let Some(metadata) = &helper.metadata else {
                return false;
            };
            if metadata.emitted_operation_id != primary_metadata.emitted_operation_id
                || metadata.representation != primary_metadata.representation
                || metadata.success_statuses != primary_metadata.success_statuses
                || metadata.request_discriminators != primary_metadata.request_discriminators
                || metadata.stream_abi != primary_metadata.stream_abi
                || helper.return_type != primary.return_type
                || helper.success_type != primary.success_type
                || helper.stream != primary.stream
            {
                return false;
            }

            let filename_parameters: Vec<_> = helper
                .parameters
                .iter()
                .filter(|parameter| parameter.name == "multipart_filenames")
                .collect();
            if filename_parameters.len() != 1
                || filename_parameters[0].type_name != "&[(&str, &str)]"
            {
                return false;
            }

            let helper_parameters: BTreeSet<_> = helper
                .parameters
                .iter()
                .filter(|parameter| parameter.name != "multipart_filenames")
                .map(|parameter| (parameter.name.as_str(), parameter.type_name.as_str()))
                .collect();
            helper_parameters.len() + 1 == helper.parameters.len()
                && helper_parameters == primary_parameters
        })
        .collect();

    if matching.len() != 1 {
        return Err("bindings.multipart_filenames_signature_mismatch");
    }
    Ok(Some((matching[0].0.as_str(), matching[0].1)))
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestUnionBranch {
    pub raw_variant: String,
    pub schema: String,
    pub raw_payload: String,
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

fn binary_rust_type(syntax: &Type) -> bool {
    syntax.spelling == "String"
        || syntax.spelling == "bytes::Bytes"
        || syntax
            .unary("Vec")
            .is_some_and(|inner| inner.spelling == "u8")
}

fn scalar_enum_matches_schema(schema: &Value, raw: &str, bindings: &Bindings) -> bool {
    let Some(values) = schema.get("enum").and_then(Value::as_array) else {
        return false;
    };
    let Some(values) = values.iter().map(Value::as_str).collect::<Option<Vec<_>>>() else {
        return false;
    };
    let wire = values.into_iter().collect::<BTreeSet<_>>();
    if wire.is_empty() {
        return false;
    }
    bindings.enums.get(raw).is_some_and(|variants| {
        variants.iter().all(|variant| {
            variant.payload.is_none()
                && variant
                    .wire_name
                    .as_deref()
                    .is_some_and(|value| wire.contains(value))
        }) && variants
            .iter()
            .filter_map(|variant| variant.wire_name.as_deref())
            .collect::<BTreeSet<_>>()
            == wire
    })
}

fn scalar_const_enum_matches_schema(schema: &Value, raw: &str, bindings: &Bindings) -> bool {
    let Some(value) = schema.get("const").and_then(Value::as_str) else {
        return false;
    };
    schema.get("type").and_then(Value::as_str) == Some("string")
        && schema.as_object().is_some_and(|fields| {
            fields.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "type"
                        | "const"
                        | "title"
                        | "description"
                        | "default"
                        | "example"
                        | "examples"
                        | "deprecated"
                        | "$comment"
                )
            })
        })
        && bindings.enums.get(raw).is_some_and(|variants| {
            variants.len() == 1
                && variants[0].payload.is_none()
                && variants[0].wire_name.as_deref() == Some(value)
        })
}

fn map_value_type<'a>(syntax: &'a Type, bindings: &'a Bindings) -> Option<Type> {
    if syntax.constructor.as_deref() == Some("std::collections::BTreeMap")
        && syntax.arguments.len() == 2
        && syntax.arguments[0].spelling == "String"
    {
        return Some(syntax.arguments[1].clone());
    }
    let fields = bindings.structs.get(&syntax.spelling)?;
    if fields.len() != 1 {
        return None;
    }
    let field = parse_type(&fields[0].type_name).ok()?;
    (field.constructor.as_deref() == Some("std::collections::BTreeMap")
        && field.arguments.len() == 2
        && field.arguments[0].spelling == "String")
        .then(|| field.arguments[1].clone())
}

/// The generated raw fallback for a map of primitive JSON values is intentionally
/// broader than the OpenAPI validation schema. It is a lossless transport
/// representation, not a claim that Rust types enforce server-side validation.
/// Keep this fallback restricted to a canonical flattened JSON map and a
/// fully inspected finite union of JSON scalar/flat-array alternatives.
fn lossless_primitive_json_map(schema: &Value, syntax: &Type, bindings: &Bindings) -> bool {
    let Some(fields) = bindings.structs.get(&syntax.spelling) else {
        return false;
    };
    if fields.len() != 1
        || fields[0].name != "additional_properties"
        || fields[0].wire_name.is_some()
        || fields[0].type_name != "std::collections::BTreeMap<String, serde_json::Value>"
    {
        return false;
    }
    let Some(schema) = schema.as_object() else {
        return false;
    };
    if schema.keys().any(|key| {
        !matches!(
            key.as_str(),
            "anyOf" | "title" | "description" | "deprecated" | "example" | "examples"
        )
    }) {
        return false;
    }
    let Some(branches) = schema
        .get("anyOf")
        .and_then(Value::as_array)
        .filter(|branches| branches.len() >= 2)
    else {
        return false;
    };
    branches.iter().all(|branch| {
        let Some(object) = branch.as_object() else {
            return false;
        };
        let scalar = |value: &Value| {
            value.as_object().is_some_and(|object| {
                let kind = object.get("type").and_then(Value::as_str);
                matches!(kind, Some("boolean" | "integer" | "number" | "string"))
                    && object.keys().all(|key| {
                        matches!(
                            key.as_str(),
                            "type" | "format" | "title" | "description" | "deprecated"
                        )
                    })
                    && object.get("format").is_none_or(|format| {
                        kind == Some("string") && format.as_str() == Some("date-time")
                    })
            })
        };
        match object.get("type").and_then(Value::as_str) {
            Some("array") => {
                object.keys().all(|key| {
                    matches!(
                        key.as_str(),
                        "type" | "items" | "title" | "description" | "deprecated"
                    )
                }) && object.get("items").is_some_and(scalar)
            }
            _ => scalar(branch),
        }
    })
}

/// Identical alternatives in anyOf are semantically redundant; oneOf is not.
pub(crate) fn redundant_any_of_alternative(schema: &Value) -> Option<&Value> {
    let object = schema.as_object()?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "anyOf"
                | "title"
                | "description"
                | "example"
                | "examples"
                | "default"
                | "deprecated"
                | "$comment"
        )
    }) {
        return None;
    }
    let branches = object.get("anyOf")?.as_array()?;
    let first = branches.first()?;
    (branches.len() >= 2 && branches.iter().all(|branch| branch == first)).then_some(first)
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

    // Heap allocation is serialization-transparent. Keep recursive cycle
    // detection on the underlying semantic type rather than on Box itself.
    if let Some(inner) = syntax.unary("Box") {
        return type_matches_schema(schema, inner, bindings, seen_aliases);
    }

    // A referenced component and the emitted Rust symbol share canonical identity.
    // Preserve that identity inside collections as well as at a response root.
    if let Some(reference) = ref_name(schema) {
        return syntax.spelling == reference
            && (bindings.structs.contains_key(reference)
                || bindings.enums.contains_key(reference)
                || bindings.aliases.contains_key(reference));
    }

    if let Some(non_null) = nullable_schema(schema) {
        return syntax
            .unary("Option")
            .is_some_and(|inner| type_matches_schema(non_null, inner, bindings, seen_aliases));
    }
    if syntax.unary("Option").is_some() {
        return false;
    }

    if let Some(alternative) = redundant_any_of_alternative(schema) {
        return type_matches_schema(alternative, syntax, bindings, seen_aliases);
    }

    if let Some(object) = schema.as_object() {
        let union_keys =
            usize::from(object.contains_key("oneOf")) + usize::from(object.contains_key("anyOf"));
        let annotation_only_union = union_keys == 1
            && object.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "oneOf"
                        | "anyOf"
                        | "title"
                        | "description"
                        | "default"
                        | "example"
                        | "examples"
                        | "deprecated"
                        | "discriminator"
                        | "$comment"
                )
            });
        if annotation_only_union {
            // Exactly one oneOf alternative has precisely that alternative's
            // validation semantics. Keep all sibling constraints fail-closed.
            if let Some(branch) = object
                .get("oneOf")
                .and_then(Value::as_array)
                .filter(|branches| branches.len() == 1)
                .and_then(|branches| branches.first())
            {
                return type_matches_schema(branch, syntax, bindings, seen_aliases);
            }
            let branches = object
                .get("oneOf")
                .or_else(|| object.get("anyOf"))
                .and_then(Value::as_array)
                .filter(|branches| branches.len() >= 2);
            let Some(branches) = branches else {
                return false;
            };
            let Some(variants) = bindings.enums.get(&syntax.spelling) else {
                return false;
            };
            if variants.len() != branches.len()
                || variants.iter().any(|variant| variant.payload.is_none())
            {
                return false;
            }

            let guard = format!("@union:{}", syntax.spelling);
            if !seen_aliases.insert(guard.clone()) {
                return false;
            }
            let mut used = BTreeSet::new();
            let matched = branches.iter().all(|branch| {
                let matches = variants
                    .iter()
                    .enumerate()
                    .filter(|(_, variant)| {
                        variant.payload.as_deref().is_some_and(|payload| {
                            parse_type(payload).ok().is_some_and(|payload_syntax| {
                                type_matches_schema(branch, &payload_syntax, bindings, seen_aliases)
                            })
                        })
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                matches.len() == 1 && used.insert(matches[0])
            }) && used.len() == variants.len();
            seen_aliases.remove(&guard);
            return matched;
        }
    }

    // Annotation-only OpenAPI schemas place no constraints on the JSON value.
    if schema.as_object().is_some_and(|object| {
        object.keys().all(|key| {
            matches!(
                key.as_str(),
                "title" | "description" | "example" | "examples" | "deprecated" | "$comment"
            )
        })
    }) {
        return syntax.spelling == "serde_json::Value";
    }

    if schema.get("const").is_some() {
        return scalar_const_enum_matches_schema(schema, &syntax.spelling, bindings);
    }

    match schema.get("type").and_then(Value::as_str) {
        Some("string") => {
            let format = schema.get("format").and_then(Value::as_str);
            if format == Some("binary") {
                binary_rust_type(syntax)
            } else {
                syntax.spelling == "String"
                    || matches!(
                        (format, syntax.spelling.as_str()),
                        (Some("uuid"), "uuid::Uuid")
                            | (Some("date-time"), "chrono::DateTime<chrono::Utc>")
                            | (Some("date"), "chrono::NaiveDate")
                            | (Some("uri"), "url::Url")
                    )
                    || scalar_enum_matches_schema(schema, &syntax.spelling, bindings)
            }
        }
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
                        map_value_type(syntax, bindings).is_some_and(|value_type| {
                            if *additional == Value::Bool(true) {
                                value_type.spelling == "serde_json::Value"
                            } else if lossless_primitive_json_map(additional, syntax, bindings) {
                                true
                            } else {
                                type_matches_schema(additional, &value_type, bindings, seen_aliases)
                            }
                        })
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

/// Match array-union responses against the exact emitted Rust union payloads.
///
/// OpenAPI component identifiers need not match Rust symbols: a generated
/// backend may rename a component while preserving its fully proven structure.
/// Each emitted union payload must map to exactly one wire array alternative.
pub(crate) fn response_array_union_matches(
    openapi: &OpenApiIndex,
    schema: &Value,
    raw_union: &str,
    bindings: &Bindings,
) -> bool {
    let Some(object) = schema.as_object() else {
        return false;
    };
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "anyOf"
                | "oneOf"
                | "title"
                | "description"
                | "deprecated"
                | "example"
                | "examples"
                | "default"
        )
    }) {
        return false;
    }
    let Some(branches) = object
        .get("oneOf")
        .or_else(|| object.get("anyOf"))
        .and_then(Value::as_array)
        .filter(|branches| branches.len() >= 2)
    else {
        return false;
    };
    let Some(variants) = bindings.enums.get(raw_union) else {
        return false;
    };
    if variants.len() != branches.len() || variants.iter().any(|variant| variant.payload.is_none())
    {
        return false;
    }
    let mut used = BTreeSet::new();
    let matched = branches.iter().all(|branch| {
        let Some(items) = branch
            .as_object()
            .filter(|object| {
                object.get("type").and_then(Value::as_str) == Some("array")
                    && object.keys().all(|key| {
                        matches!(
                            key.as_str(),
                            "type" | "items" | "title" | "description" | "deprecated"
                        )
                    })
            })
            .and_then(|object| object.get("items"))
        else {
            return false;
        };
        let matches = variants
            .iter()
            .enumerate()
            .filter(|(_, variant)| {
                let Some(payload) = variant.payload.as_deref() else {
                    return false;
                };
                let mut current = payload.to_owned();
                let mut seen = BTreeSet::new();
                while let Some(alias) = bindings.aliases.get(&current) {
                    if !seen.insert(current.clone()) {
                        return false;
                    }
                    current = alias.clone();
                }
                let Ok(syntax) = parse_type(&current) else {
                    return false;
                };
                let Some(inner) = syntax.unary("Vec") else {
                    return false;
                };
                if let Some(reference) = ref_name(items) {
                    return (inner.spelling == reference
                        && (bindings.structs.contains_key(reference)
                            || bindings.enums.contains_key(reference)
                            || bindings.aliases.contains_key(reference)))
                        || request_object_matches(openapi, reference, &inner.spelling, bindings);
                }
                canonical_unconstrained_map_branch(items, &inner.spelling, bindings)
                    || rust_type_matches_schema(items, &inner.spelling, bindings)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        matches.len() == 1 && used.insert(matches[0])
    });
    matched && used.len() == variants.len()
}

/// Normalize an exactly nullable two-branch request union. We do not drop
/// additional constraints or pretend that the null variant is an enum payload.
pub(crate) fn nullable_request_union(schema: &Value) -> Option<Value> {
    let object = schema.as_object()?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "anyOf"
                | "title"
                | "description"
                | "deprecated"
                | "example"
                | "examples"
                | "default"
                | "additionalProperties"
        )
    }) || object
        .get("additionalProperties")
        .is_some_and(|value| value != &Value::Bool(true))
    {
        // additionalProperties:true has no validation effect. Any restrictive
        // sibling (including additionalProperties:false) still fails closed.
        return None;
    }
    let branches = schema.get("anyOf")?.as_array()?;
    if branches.len() != 3
        || branches
            .iter()
            .filter(|branch| **branch == serde_json::json!({"type": "null"}))
            .count()
            != 1
    {
        return None;
    }
    let non_null: Vec<_> = branches
        .iter()
        .filter(|branch| **branch != serde_json::json!({"type": "null"}))
        .cloned()
        .collect();
    Some(serde_json::json!({"anyOf": non_null}))
}

/// Normalize a typed request property using the legacy OpenAPI nullable flag.
/// The flag is not JSON Schema 2020-12 syntax, but some OpenAPI 3.1 producers
/// retain it and the pinned raw generator emits two Option layers for an
/// optional nullable field. Never discard composition/union constraints.
pub(crate) fn legacy_nullable_request_property(schema: &Value) -> Option<Value> {
    let object = schema.as_object()?;
    if object.get("nullable") != Some(&Value::Bool(true))
        || (!object.contains_key("type") && !object.contains_key("$ref"))
        || ["oneOf", "anyOf", "allOf"]
            .iter()
            .any(|key| object.contains_key(*key))
    {
        return None;
    }
    if object.contains_key("$ref")
        && (object.contains_key("type")
            || object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "$ref"
                        | "nullable"
                        | "title"
                        | "description"
                        | "deprecated"
                        | "example"
                        | "examples"
                        | "default"
                        | "$comment"
                )
            }))
    {
        // Constraints alongside a reference must not be lost by ref_name().
        return None;
    }
    let mut non_null = object.clone();
    non_null.remove("nullable");
    Some(Value::Object(non_null))
}

fn canonical_unconstrained_map_branch(schema: &Value, raw: &str, bindings: &Bindings) -> bool {
    let Some(shape) = schema.as_object() else {
        return false;
    };
    if shape.get("type").and_then(Value::as_str) != Some("object")
        || shape.get("additionalProperties") != Some(&Value::Bool(true))
        || shape.contains_key("properties")
        || shape.keys().any(|key| {
            !matches!(
                key.as_str(),
                "type" | "additionalProperties" | "title" | "description" | "deprecated"
            )
        })
    {
        return false;
    }
    flattened_json_response_object_matches(
        &serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true
        }),
        raw,
        bindings,
    )
}

fn rust_option_core(type_name: &str) -> Option<(Type, usize)> {
    let mut syntax = parse_type(type_name).ok()?;
    let mut depth = 0;
    while let Some(inner) = syntax.unary("Option") {
        depth += 1;
        syntax = inner.clone();
    }
    Some((syntax, depth))
}

fn union_branches(schema: &Value) -> Option<&Vec<Value>> {
    let branches = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))?
        .as_array()?;
    (branches.len() >= 2).then_some(branches)
}

fn transparent_box_raw(type_name: &str) -> Option<String> {
    let mut syntax = parse_type(type_name).ok()?;
    while let Some(inner) = syntax.unary("Box") {
        syntax = inner.clone();
    }
    Some(syntax.spelling)
}

fn request_union_mapping_inner(
    openapi: &OpenApiIndex,
    schema: &Value,
    raw_union: &str,
    bindings: &Bindings,
    seen: &mut BTreeSet<(String, String)>,
) -> Option<Vec<RequestUnionBranch>> {
    let branches = union_branches(schema)?;
    let references = branches
        .iter()
        .map(|branch| ref_name(branch).map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    let reference_set: BTreeSet<_> = references.iter().collect();
    if reference_set.len() != references.len() {
        return None;
    }

    let variants = bindings.enums.get(raw_union)?;
    if variants.len() != references.len()
        || variants.iter().any(|variant| variant.payload.is_none())
    {
        return None;
    }

    let mut used = BTreeSet::new();
    let mut mapping = Vec::new();
    for reference in references {
        let matches = variants
            .iter()
            .enumerate()
            .filter(|(_, variant)| {
                variant.payload.as_ref().is_some_and(|payload| {
                    let raw = transparent_box_raw(payload).unwrap_or_else(|| payload.clone());
                    request_object_matches_inner(openapi, &reference, &raw, bindings, seen)
                })
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 || !used.insert(matches[0].0) {
            return None;
        }
        let (_, variant) = matches[0];
        mapping.push(RequestUnionBranch {
            raw_variant: variant.name.clone(),
            schema: reference,
            raw_payload: variant.payload.clone()?,
        });
    }

    (used.len() == variants.len()).then_some(mapping)
}

pub(crate) fn request_union_mapping(
    openapi: &OpenApiIndex,
    schema: &Value,
    raw_union: &str,
    bindings: &Bindings,
) -> Option<Vec<RequestUnionBranch>> {
    request_union_mapping_inner(openapi, schema, raw_union, bindings, &mut BTreeSet::new())
}

fn unconstrained_request_object_branch(schema: &Value) -> bool {
    let Some(object) = schema.as_object() else {
        return false;
    };
    if object.get("type").and_then(Value::as_str) != Some("object")
        || object.get("properties").is_some_and(|properties| {
            !properties
                .as_object()
                .is_some_and(|properties| properties.is_empty())
        })
        || object
            .get("additionalProperties")
            .is_some_and(|additional| additional != &Value::Bool(true))
    {
        return false;
    }
    object.keys().all(|key| {
        matches!(
            key.as_str(),
            "type"
                | "properties"
                | "additionalProperties"
                | "title"
                | "description"
                | "deprecated"
                | "example"
                | "examples"
                | "default"
                | "$comment"
        )
    })
}

fn redundant_unconstrained_object_any_of_matches(
    schema: &Value,
    variants: &[crate::contracts::VariantBinding],
    bindings: &Bindings,
) -> bool {
    let Some(object) = schema.as_object() else {
        return false;
    };
    if object.contains_key("oneOf")
        || object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "anyOf"
                    | "title"
                    | "description"
                    | "deprecated"
                    | "example"
                    | "examples"
                    | "default"
                    | "$comment"
            )
        })
    {
        return false;
    }
    let Some(branches) = object
        .get("anyOf")
        .and_then(Value::as_array)
        .filter(|branches| branches.len() >= 2)
    else {
        return false;
    };
    if !branches.iter().all(unconstrained_request_object_branch)
        || variants.is_empty()
        || variants.iter().any(|variant| variant.payload.is_none())
    {
        return false;
    }
    let canonical = serde_json::json!({
        "type": "object",
        "additionalProperties": true
    });
    variants.iter().all(|variant| {
        variant
            .payload
            .as_deref()
            .is_some_and(|payload| rust_type_matches_schema(&canonical, payload, bindings))
    })
}

fn request_union_matches_inner(
    openapi: &OpenApiIndex,
    schema: &Value,
    raw_union: &str,
    bindings: &Bindings,
    seen: &mut BTreeSet<(String, String)>,
) -> bool {
    let Some(branches) = union_branches(schema) else {
        return false;
    };
    let Some(variants) = bindings.enums.get(raw_union) else {
        return false;
    };
    if redundant_unconstrained_object_any_of_matches(schema, variants, bindings) {
        return true;
    }
    if variants.len() != branches.len() || variants.iter().any(|variant| variant.payload.is_none())
    {
        return false;
    }

    let branch_matches = |branch: &Value, payload: &str, seen: &mut BTreeSet<(String, String)>| {
        if let Some(reference) = ref_name(branch) {
            let Ok(referenced) = openapi.schema(reference) else {
                return false;
            };
            let raw = transparent_box_raw(payload).unwrap_or_else(|| payload.to_owned());
            if union_branches(referenced).is_some() {
                return request_union_matches_inner(openapi, referenced, &raw, bindings, seen);
            }
            if referenced_request_object(openapi, reference, referenced).is_some() {
                return request_object_matches_inner(openapi, reference, &raw, bindings, seen);
            }
            return rust_type_matches_schema(referenced, payload, bindings);
        }
        if branch.get("properties").is_some() {
            return request_object_value_matches(openapi, branch, payload, bindings, seen);
        }
        canonical_unconstrained_map_branch(branch, payload, bindings)
            || rust_type_matches_schema(branch, payload, bindings)
    };

    let mut used = BTreeSet::new();
    for branch in branches {
        let matches = variants
            .iter()
            .enumerate()
            .filter(|(_, variant)| {
                variant.payload.as_deref().is_some_and(|payload| {
                    let mut branch_seen = seen.clone();
                    branch_matches(branch, payload, &mut branch_seen)
                })
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if matches.len() != 1 || !used.insert(matches[0]) {
            return false;
        }
    }
    used.len() == variants.len()
}

pub(crate) fn request_union_matches(
    openapi: &OpenApiIndex,
    schema: &Value,
    raw_union: &str,
    bindings: &Bindings,
) -> bool {
    let without_null = nullable_request_union(schema);
    request_union_matches_inner(
        openapi,
        without_null.as_ref().unwrap_or(schema),
        raw_union,
        bindings,
        &mut BTreeSet::new(),
    )
}

fn request_union_collection<'a>(
    openapi: &'a OpenApiIndex,
    schema: &'a Value,
    core: &'a Type,
) -> Option<(&'a Value, &'a str)> {
    if schema.get("type").and_then(Value::as_str) != Some("array") {
        return None;
    }
    let items = schema.get("items")?;
    let inner = core.unary("Vec")?;
    if inner.kind != TypeKind::Opaque {
        return None;
    }

    if union_branches(items).is_some() {
        return Some((items, inner.spelling.as_str()));
    }
    let reference = ref_name(items)?;
    let referenced = openapi.schema(reference).ok()?;
    union_branches(referenced)
        .is_some()
        .then_some((referenced, inner.spelling.as_str()))
}

/// Only route referenced request shapes to object projection when they declare
/// named properties (possibly through allOf). A map schema with solely
/// additionalProperties is a raw map, not a nested request wrapper.
pub(crate) fn referenced_request_object(
    openapi: &OpenApiIndex,
    reference: &str,
    source: &Value,
) -> Option<Value> {
    if source.get("properties").is_none()
        && source.get("allOf").is_none()
        && source.get("$ref").is_none()
    {
        return None;
    }
    let composed = openapi.object_schema(reference).ok()?;
    if source.get("properties").is_some()
        || composed
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|properties| !properties.is_empty())
    {
        Some(composed)
    } else {
        None
    }
}

fn single_all_of_annotation_branch(schema: &Value) -> Option<&Value> {
    let object = schema.as_object()?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "allOf" | "title" | "description" | "deprecated" | "example" | "examples" | "default"
        )
    }) {
        return None;
    }
    let branches = object.get("allOf")?.as_array()?;
    (branches.len() == 1).then(|| &branches[0])
}

fn request_object_value_matches(
    openapi: &OpenApiIndex,
    schema: &Value,
    raw: &str,
    bindings: &Bindings,
    seen: &mut BTreeSet<(String, String)>,
) -> bool {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return false;
    };
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
        return false;
    }

    let Some(fields) = bindings.structs.get(raw) else {
        return false;
    };
    // A canonical flattened map captures additional JSON members; it is not
    // itself a named OpenAPI property. All actual properties are still checked
    // recursively below using their exact wire and Rust types.
    let flattened = flattened_json_response_object_matches(schema, raw, bindings);
    let by_name: BTreeMap<_, _> = fields
        .iter()
        .filter(|field| !flattened || field.name != "additional_properties")
        .map(|field| {
            (
                field
                    .wire_name
                    .as_deref()
                    .unwrap_or_else(|| field.name.strip_prefix("r#").unwrap_or(&field.name)),
                field,
            )
        })
        .collect();
    if by_name.len() + usize::from(flattened) != fields.len()
        || by_name.keys().copied().collect::<BTreeSet<_>>()
            != properties
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
    {
        return false;
    }

    for (name, property) in properties {
        let property = single_all_of_annotation_branch(property).unwrap_or(property);
        let field = by_name[name.as_str()];
        let Some((core, option_depth)) = rust_option_core(&field.type_name) else {
            return false;
        };
        let referenced = ref_name(property).and_then(|reference| openapi.schema(reference).ok());
        let nullable_wire = nullable_request_union(property)
            .or_else(|| referenced.and_then(nullable_request_union))
            .or_else(|| nullable_schema(property).cloned())
            .or_else(|| referenced.and_then(nullable_schema).cloned())
            .or_else(|| legacy_nullable_request_property(property))
            .or_else(|| referenced.and_then(legacy_nullable_request_property));
        let (wire, nullable) = nullable_wire
            .as_ref()
            .map(|schema| (schema, true))
            .unwrap_or((property, false));
        let expected_depth = usize::from(!required.contains(name.as_str())) + usize::from(nullable);
        if option_depth != expected_depth {
            return false;
        }

        if let Some(reference) = ref_name(wire) {
            let Some(referenced) = openapi.schema(reference).ok() else {
                return false;
            };
            let matches = if union_branches(referenced).is_some() {
                core.kind == TypeKind::Opaque
                    && request_union_matches_inner(
                        openapi,
                        referenced,
                        &core.spelling,
                        bindings,
                        seen,
                    )
            } else if referenced_request_object(openapi, reference, referenced).is_some() {
                core.kind == TypeKind::Opaque
                    && request_object_matches_inner(
                        openapi,
                        reference,
                        &core.spelling,
                        bindings,
                        seen,
                    )
            } else {
                type_matches_schema(referenced, &core, bindings, &mut BTreeSet::new())
            };
            if !matches {
                return false;
            }
            continue;
        }

        if union_branches(wire).is_some() {
            if redundant_any_of_alternative(wire).is_some_and(|alternative| {
                type_matches_schema(alternative, &core, bindings, &mut BTreeSet::new())
            }) {
                continue;
            }
            if core.kind != TypeKind::Opaque
                || !request_union_matches_inner(openapi, wire, &core.spelling, bindings, seen)
            {
                return false;
            }
            continue;
        }

        if let Some((union_schema, raw_union)) = request_union_collection(openapi, wire, &core) {
            if !request_union_matches_inner(openapi, union_schema, raw_union, bindings, seen) {
                return false;
            }
            continue;
        }

        if wire.get("properties").is_some() {
            if core.kind != TypeKind::Opaque
                || !request_object_value_matches(openapi, wire, &core.spelling, bindings, seen)
            {
                return false;
            }
        } else if !type_matches_schema(wire, &core, bindings, &mut BTreeSet::new()) {
            return false;
        }
    }

    true
}

fn request_object_matches_inner(
    openapi: &OpenApiIndex,
    schema_name: &str,
    raw: &str,
    bindings: &Bindings,
    seen: &mut BTreeSet<(String, String)>,
) -> bool {
    let pair = (schema_name.to_owned(), raw.to_owned());
    if !seen.insert(pair.clone()) {
        return false;
    }

    let matched = if let Some(alias) = bindings.aliases.get(raw) {
        request_object_matches_inner(openapi, schema_name, alias, bindings, seen)
    } else {
        openapi
            .object_schema(schema_name)
            .ok()
            .is_some_and(|schema| {
                request_object_value_matches(openapi, &schema, raw, bindings, seen)
            })
    };

    seen.remove(&pair);
    matched
}

pub(crate) fn request_optional_boolean_field(
    openapi: &OpenApiIndex,
    schema_name: &str,
    raw: &str,
    field_name: &str,
    bindings: &Bindings,
) -> bool {
    let Ok(schema) = openapi.object_schema(schema_name) else {
        return false;
    };
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return false;
    };
    let Some(property) = properties.get(field_name) else {
        return false;
    };
    if property.get("type").and_then(Value::as_str) != Some("boolean") {
        return false;
    }
    if schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|required| required == field_name)
    {
        return false;
    }

    let Some(raw_field) = bindings.structs.get(raw).and_then(|fields| {
        fields
            .iter()
            .find(|field| field.name.strip_prefix("r#").unwrap_or(&field.name) == field_name)
    }) else {
        return false;
    };
    let Ok(syntax) = parse_type(&raw_field.type_name) else {
        return false;
    };
    syntax
        .unary("Option")
        .is_some_and(|inner| inner.spelling == "bool" && inner.unary("Option").is_none())
}

pub(crate) fn request_object_matches(
    openapi: &OpenApiIndex,
    schema_name: &str,
    raw: &str,
    bindings: &Bindings,
) -> bool {
    request_object_matches_inner(openapi, schema_name, raw, bindings, &mut BTreeSet::new())
}

/// Prove a request object whose canonical call shape fixes optional Boolean
/// discriminator fields to exact wire constants.
///
/// The ordinary structural proof remains authoritative. This fallback only
/// removes a Boolean `const` or single-value Boolean `enum` after generator-
/// owned operation metadata proves the same field/value and the raw request
/// exposes exactly `Option<bool>`. Every other request field is then checked
/// by the normal recursive structural matcher.
pub(crate) fn request_object_matches_with_discriminators(
    openapi: &OpenApiIndex,
    schema_name: &str,
    raw: &str,
    bindings: &Bindings,
    discriminators: &[RequestDiscriminatorBinding],
) -> bool {
    if request_object_matches(openapi, schema_name, raw, bindings) {
        return true;
    }
    if discriminators.is_empty() {
        return false;
    }

    let Ok(mut schema) = openapi.object_schema(schema_name) else {
        return false;
    };
    let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) else {
        return false;
    };

    let mut normalized = false;
    let mut seen = BTreeSet::new();
    for discriminator in discriminators {
        if discriminator.rust_access_path.len() != 1
            || discriminator.field_required
            || discriminator.field_nullable
            || discriminator.field_tri_state
            || !matches!(
                discriminator.rust_value_type.as_str(),
                "bool" | "Option<bool>"
            )
        {
            return false;
        }
        let raw_field = discriminator.rust_access_path[0]
            .strip_prefix("r#")
            .unwrap_or(&discriminator.rust_access_path[0]);
        if raw_field != discriminator.wire_name || !seen.insert(raw_field.to_owned()) {
            return false;
        }
        let RequestDiscriminatorValue::Bool(value) = discriminator.value else {
            return false;
        };
        if !request_optional_boolean_field(openapi, schema_name, raw, raw_field, bindings) {
            return false;
        }

        let Some(property) = properties.get_mut(raw_field).and_then(Value::as_object_mut) else {
            return false;
        };
        let const_matches = property
            .get("const")
            .is_some_and(|candidate| candidate == &Value::Bool(value));
        let enum_matches = property
            .get("enum")
            .and_then(Value::as_array)
            .is_some_and(|values| values.as_slice() == [Value::Bool(value)]);
        if property.contains_key("const") && !const_matches {
            return false;
        }
        if property.contains_key("enum") && !enum_matches {
            return false;
        }
        if const_matches {
            property.remove("const");
            normalized = true;
        }
        if enum_matches {
            property.remove("enum");
            normalized = true;
        }
    }

    normalized
        && request_object_value_matches(openapi, &schema, raw, bindings, &mut BTreeSet::new())
}

pub(crate) fn object_value_matches(
    openapi: &OpenApiIndex,
    schema: &Value,
    raw: &str,
    bindings: &Bindings,
) -> bool {
    request_object_value_matches(openapi, schema, raw, bindings, &mut BTreeSet::new())
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

pub(crate) fn scalar_named_object_matches(
    openapi: &OpenApiIndex,
    schema_name: &str,
    raw: &str,
    bindings: &Bindings,
) -> bool {
    openapi
        .object_schema(schema_name)
        .ok()
        .and_then(|schema| scalar_object_shape(&schema))
        .zip(raw_scalar_struct_shape(bindings, raw))
        .is_some_and(|(wire, actual)| wire == actual)
}

fn sse_envelope_payloads(schema: &Value) -> Option<Vec<&str>> {
    let properties = schema.get("properties").and_then(Value::as_object)?;
    let data = properties.get("data")?;
    let data_required = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|field| field == "data");
    let has_sse_metadata = properties.keys().any(|field| field != "data");
    if !data_required && !has_sse_metadata {
        return None;
    }
    for (field, value) in properties {
        let valid = match field.as_str() {
            "data" => true,
            "event" | "id" => direct_scalar_schema(value) == Some(ScalarKind::String),
            "retry" => direct_scalar_schema(value) == Some(ScalarKind::Integer),
            _ => false,
        };
        if !valid {
            return None;
        }
    }

    if let Some(reference) = ref_name(data) {
        return Some(vec![reference]);
    }

    // Typed facade unions use only OpenAPI oneOf. anyOf does not prove that
    // exactly one payload schema applies, while serde's untagged decoder would
    // otherwise silently choose the first matching branch.
    let branches = data.get("oneOf").and_then(Value::as_array)?;
    if branches.len() < 2 {
        return None;
    }
    let payloads = branches.iter().map(ref_name).collect::<Option<Vec<_>>>()?;
    let unique: BTreeSet<_> = payloads.iter().copied().collect();
    (unique.len() == payloads.len()).then_some(payloads)
}

pub(crate) fn sse_payload_schema_names(
    openapi: &OpenApiIndex,
    schema: &Value,
) -> Option<Vec<String>> {
    if let Some(root) = ref_name(schema) {
        let resolved = openapi.schema(root).ok()?;
        return sse_envelope_payloads(resolved)
            .map(|payloads| payloads.into_iter().map(str::to_owned).collect())
            .or_else(|| Some(vec![root.to_owned()]));
    }
    sse_envelope_payloads(schema).map(|payloads| payloads.into_iter().map(str::to_owned).collect())
}

pub(crate) fn sse_payload_schema_name(openapi: &OpenApiIndex, schema: &Value) -> Option<String> {
    let mut payloads = sse_payload_schema_names(openapi, schema)?;
    (payloads.len() == 1).then(|| payloads.remove(0))
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

pub(crate) fn plain_string_json_alias_matches(
    schema: &Value,
    raw_success: &str,
    bindings: &Bindings,
) -> bool {
    schema.as_object().is_some_and(|fields| {
        fields.get("type").and_then(Value::as_str) == Some("string")
            && fields.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "type" | "title" | "description" | "examples" | "default"
                )
            })
    }) && bindings
        .aliases
        .get(raw_success)
        .is_some_and(|alias| alias == "String")
}

/// Prove a response object with one or more string-constant fields represented
/// by exact one-variant raw enums, while checking every other scalar field and
/// preserving the required/nullable wrapper depth.
pub(crate) fn constant_enum_response_object_matches(
    schema: &Value,
    raw: &str,
    bindings: &Bindings,
) -> bool {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return false;
    };
    if schema
        .get("additionalProperties")
        .is_some_and(|extra| extra != &Value::Bool(false))
    {
        return false;
    }
    let Some(fields) = bindings.structs.get(raw) else {
        return false;
    };
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
        return false;
    }
    let mut seen = BTreeSet::new();
    let mut constants = 0;
    for field in fields {
        let name = field.name.strip_prefix("r#").unwrap_or(&field.name);
        let wire_name = field.wire_name.as_deref().unwrap_or(name);
        if !seen.insert(wire_name) {
            return false;
        }
        let Some(property) = properties.get(wire_name) else {
            return false;
        };
        let Some((core, depth)) = rust_option_core(&field.type_name) else {
            return false;
        };
        let (wire, nullable) = nullable_schema(property)
            .map(|value| (value, true))
            .unwrap_or((property, false));
        if depth != usize::from(!required.contains(wire_name)) + usize::from(nullable) {
            return false;
        }
        if let Some(constant) = wire.get("const") {
            let Some(value) = constant.as_str() else {
                return false;
            };
            if wire.get("type").and_then(Value::as_str) != Some("string")
                || !bindings.enums.get(&core.spelling).is_some_and(|variants| {
                    variants.len() == 1
                        && variants[0].payload.is_none()
                        && variants[0].wire_name.as_deref() == Some(value)
                })
            {
                return false;
            }
            constants += 1;
        } else {
            let Some((kind, _)) = scalar_schema(wire) else {
                return false;
            };
            if rust_scalar(&core.spelling) != Some((kind, 0)) {
                return false;
            }
        }
    }
    constants > 0 && seen.len() == properties.len()
}

/// An object may carry additional JSON members through a flattened raw map.
pub(crate) fn flattened_json_response_object_matches(
    schema: &Value,
    raw: &str,
    bindings: &Bindings,
) -> bool {
    if schema.get("type").and_then(Value::as_str) != Some("object")
        || schema.get("additionalProperties") != Some(&Value::Bool(true))
    {
        return false;
    }
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return false;
    };
    let Some(fields) = bindings.structs.get(raw) else {
        return false;
    };
    let mut seen = BTreeSet::new();
    let mut flattened = false;
    for field in fields {
        if field.name == "additional_properties" && field.wire_name.is_none() {
            if flattened
                || field.type_name != "std::collections::BTreeMap<String, serde_json::Value>"
            {
                return false;
            }
            flattened = true;
            continue;
        }
        let wire_name = field
            .wire_name
            .as_deref()
            .unwrap_or(field.name.strip_prefix("r#").unwrap_or(&field.name));
        if !seen.insert(wire_name) || !properties.contains_key(wire_name) {
            return false;
        }
    }
    flattened && seen.len() == properties.len()
}

pub(crate) fn object_field_names_match(schema: &Value, raw: &str, bindings: &Bindings) -> bool {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return false;
    };
    let Some(fields) = bindings.structs.get(raw) else {
        return false;
    };
    let wire_fields: BTreeSet<_> = properties.keys().map(String::as_str).collect();
    let raw_fields: BTreeSet<_> = fields
        .iter()
        .map(|field| {
            field
                .wire_name
                .as_deref()
                .unwrap_or_else(|| field.name.strip_prefix("r#").unwrap_or(&field.name))
        })
        .collect();
    fields.len() == raw_fields.len() && wire_fields == raw_fields
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

#[cfg(test)]
mod duplicate_scalar_enum_tests {
    use super::*;

    fn bindings() -> Bindings {
        serde_json::from_value(serde_json::json!({
            "schema_version": 3,
            "structs": {},
            "enums": {
                "Role": [
                    {"name": "A", "payload": null, "wire_name": "A"},
                    {"name": "Static", "payload": null, "wire_name": "static"},
                    {"name": "StaticDuplicate", "payload": null, "wire_name": "static"}
                ]
            },
            "aliases": {},
            "operations": {},
            "symbol_paths": {"Role": "crate::generated::types::Role"},
            "binding": {
                "client": {
                    "type_path": "crate::generated::Client",
                    "constructor": "new",
                    "api_key_builder": "with_api_key",
                    "base_url_builder": "with_base_url"
                },
                "type_preludes": []
            }
        }))
        .expect("canonical binding fixture")
    }

    #[test]
    fn duplicate_wire_enum_values_are_semantically_redundant() {
        let bindings = bindings();
        let schema = serde_json::json!({
            "type": "string",
            "enum": ["A", "static", "static"]
        });
        assert!(rust_type_matches_schema(&schema, "Role", &bindings));

        let source_unique = serde_json::json!({
            "type": "string",
            "enum": ["A", "static"]
        });
        assert!(rust_type_matches_schema(&source_unique, "Role", &bindings));
    }

    #[test]
    fn duplicate_wire_enum_proof_still_rejects_payloads_and_value_drift() {
        let bindings = bindings();
        let drift = serde_json::json!({
            "type": "string",
            "enum": ["A", "other", "other"]
        });
        assert!(!rust_type_matches_schema(&drift, "Role", &bindings));

        let mut payload = bindings;
        payload.enums.get_mut("Role").expect("Role")[2].payload = Some("String".into());
        let schema = serde_json::json!({
            "type": "string",
            "enum": ["A", "static", "static"]
        });
        assert!(!rust_type_matches_schema(&schema, "Role", &payload));
    }
}

#[cfg(test)]
mod referenced_collection_tests {
    use super::*;

    fn bindings() -> Bindings {
        serde_json::from_value(serde_json::json!({
            "schema_version": 3,
            "structs": {"Record": []},
            "enums": {},
            "aliases": {"RecordList": "Vec<Record>"},
            "operations": {},
            "symbol_paths": {"Record": "crate::generated::types::Record"},
            "binding": {
                "client": {
                    "type_path": "crate::generated::Client",
                    "constructor": "new",
                    "api_key_builder": "with_api_key",
                    "base_url_builder": "with_base_url"
                },
                "type_preludes": []
            }
        }))
        .expect("canonical binding fixture")
    }

    #[test]
    fn referenced_array_alias_matches_exact_binding_identity() {
        let bindings = bindings();
        let schema = serde_json::json!({
            "type": "array",
            "items": {"$ref": "#/components/schemas/Record"}
        });
        assert!(rust_type_matches_schema(&schema, "RecordList", &bindings));
        assert!(!rust_type_matches_schema(
            &serde_json::json!({
                "type": "array",
                "items": {"$ref": "#/components/schemas/DifferentRecord"}
            }),
            "RecordList",
            &bindings
        ));
        assert!(!rust_type_matches_schema(
            &schema,
            "Vec<UnboundRecord>",
            &bindings
        ));
    }
}

#[cfg(test)]
mod recursive_union_type_tests {
    use super::*;

    fn bindings() -> Bindings {
        serde_json::from_value(serde_json::json!({
            "schema_version": 3,
            "structs": {
                "Alpha": [],
                "Beta": []
            },
            "enums": {
                "ItemUnion": [
                    {"name": "Alpha", "payload": "Alpha"},
                    {"name": "Beta", "payload": "Beta"}
                ],
                "ArrayUnion": [
                    {"name": "Alphas", "payload": "AlphaList"},
                    {"name": "Betas", "payload": "BetaList"}
                ]
            },
            "aliases": {
                "ItemList": "Vec<ItemUnion>",
                "AlphaList": "Vec<Alpha>",
                "BetaList": "Vec<Beta>"
            },
            "operations": {},
            "symbol_paths": {
                "Alpha": "crate::generated::types::Alpha",
                "Beta": "crate::generated::types::Beta",
                "ItemUnion": "crate::generated::types::ItemUnion",
                "ArrayUnion": "crate::generated::types::ArrayUnion",
                "ItemList": "crate::generated::types::ItemList",
                "AlphaList": "crate::generated::types::AlphaList",
                "BetaList": "crate::generated::types::BetaList"
            },
            "binding": {
                "client": {
                    "type_path": "crate::generated::Client",
                    "constructor": "new",
                    "api_key_builder": "with_api_key",
                    "base_url_builder": "with_base_url"
                },
                "type_preludes": []
            }
        }))
        .expect("recursive union binding fixture")
    }

    #[test]
    fn proves_array_items_as_exact_named_union() {
        let schema = serde_json::json!({
            "type": "array",
            "items": {
                "anyOf": [
                    {"$ref": "#/components/schemas/Alpha"},
                    {"$ref": "#/components/schemas/Beta"}
                ]
            }
        });
        assert!(rust_type_matches_schema(&schema, "ItemList", &bindings()));
    }

    #[test]
    fn proves_union_of_arrays_bijectively() {
        let schema = serde_json::json!({
            "anyOf": [
                {"type": "array", "items": {"$ref": "#/components/schemas/Alpha"}},
                {"type": "array", "items": {"$ref": "#/components/schemas/Beta"}}
            ],
            "title": "Array response"
        });
        assert!(rust_type_matches_schema(&schema, "ArrayUnion", &bindings()));
    }

    #[test]
    fn rejects_ambiguous_union_branch_mapping() {
        let schema = serde_json::json!({
            "anyOf": [
                {"type": "array", "items": {"$ref": "#/components/schemas/Alpha"}},
                {"type": "array", "items": {"$ref": "#/components/schemas/Alpha"}}
            ]
        });
        assert!(!rust_type_matches_schema(
            &schema,
            "ArrayUnion",
            &bindings()
        ));
    }

    #[test]
    fn rejects_union_with_unmodeled_validation_sibling() {
        let schema = serde_json::json!({
            "anyOf": [
                {"$ref": "#/components/schemas/Alpha"},
                {"$ref": "#/components/schemas/Beta"}
            ],
            "type": "object"
        });
        assert!(!rust_type_matches_schema(&schema, "ItemUnion", &bindings()));
    }
}

#[cfg(test)]
mod redundant_open_object_any_of_tests {
    use super::*;
    use crate::contracts::OpenApi;

    fn fixture() -> (OpenApiIndex, Bindings) {
        let source = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "components": {"schemas": {}}
        }));
        let index = OpenApiIndex::new(&source).expect("OpenAPI index");
        let bindings = serde_json::from_value(serde_json::json!({
            "schema_version": 3,
            "structs": {
                "OpenObject": [{
                    "name": "additional_properties",
                    "type": "std::collections::BTreeMap<String, serde_json::Value>",
                    "wire_name": null
                }]
            },
            "enums": {
                "InputUnion": [
                    {"name": "Struct", "payload": "OpenObject"},
                    {"name": "Alias", "payload": "OpenObjectAlias"}
                ]
            },
            "aliases": {
                "OpenObjectAlias": "std::collections::BTreeMap<String, serde_json::Value>"
            },
            "operations": {},
            "symbol_paths": {
                "OpenObject": "crate::generated::types::OpenObject",
                "InputUnion": "crate::generated::types::InputUnion",
                "OpenObjectAlias": "crate::generated::types::OpenObjectAlias"
            },
            "binding": {
                "client": {
                    "type_path": "crate::generated::Client",
                    "constructor": "new",
                    "api_key_builder": "with_api_key",
                    "base_url_builder": "with_base_url"
                },
                "type_preludes": []
            }
        }))
        .expect("canonical binding fixture");
        (index, bindings)
    }

    fn redundant_any_of() -> Value {
        serde_json::json!({
            "anyOf": [
                {"type": "object", "additionalProperties": true},
                {"type": "object", "properties": {}}
            ]
        })
    }

    #[test]
    fn proves_semantically_redundant_unconstrained_object_any_of() {
        let (index, bindings) = fixture();
        assert!(request_union_matches(
            &index,
            &redundant_any_of(),
            "InputUnion",
            &bindings
        ));
    }

    #[test]
    fn redundant_object_proof_is_any_of_only_and_rejects_constraints() {
        let (index, bindings) = fixture();

        let one_of = serde_json::json!({
            "oneOf": [
                {"type": "object", "additionalProperties": true},
                {"type": "object", "properties": {}}
            ]
        });
        assert!(!request_union_matches(
            &index,
            &one_of,
            "InputUnion",
            &bindings
        ));

        let mut constrained = redundant_any_of();
        constrained["anyOf"][1]["properties"] = serde_json::json!({"known": {"type": "string"}});
        assert!(!request_union_matches(
            &index,
            &constrained,
            "InputUnion",
            &bindings
        ));

        let mut closed = redundant_any_of();
        closed["anyOf"][1]["additionalProperties"] = Value::Bool(false);
        assert!(!request_union_matches(
            &index,
            &closed,
            "InputUnion",
            &bindings
        ));
    }

    #[test]
    fn rejects_raw_variant_that_broadens_beyond_json_objects() {
        let (index, mut bindings) = fixture();
        bindings.enums.get_mut("InputUnion").expect("InputUnion")[1].payload =
            Some("String".into());
        assert!(!request_union_matches(
            &index,
            &redundant_any_of(),
            "InputUnion",
            &bindings
        ));
    }
}

#[cfg(test)]
mod request_scalar_union_collection_tests {
    use super::*;
    use crate::contracts::OpenApi;

    fn fixture() -> (OpenApiIndex, Bindings) {
        let source = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "components": {
                "schemas": {
                    "RoleA": {"type": "string", "enum": ["A", "M"]},
                    "RoleB": {"type": "string", "enum": ["static", "static"]},
                    "Request": {
                        "type": "object",
                        "properties": {
                            "roles": {
                                "anyOf": [
                                    {
                                        "type": "array",
                                        "items": {
                                            "anyOf": [
                                                {"$ref": "#/components/schemas/RoleA"},
                                                {"$ref": "#/components/schemas/RoleB"}
                                            ]
                                        }
                                    },
                                    {"type": "null"}
                                ]
                            }
                        }
                    }
                }
            }
        }));
        let index = OpenApiIndex::new(&source).expect("OpenAPI index");
        let bindings = serde_json::from_value(serde_json::json!({
            "schema_version": 3,
            "structs": {
                "Request": [{
                    "name": "roles",
                    "type": "Option<Option<Vec<RoleUnion>>>",
                    "wire_name": "roles"
                }]
            },
            "enums": {
                "RoleA": [
                    {"name": "A", "payload": null, "wire_name": "A"},
                    {"name": "M", "payload": null, "wire_name": "M"}
                ],
                "RoleB": [
                    {"name": "Static", "payload": null, "wire_name": "static"},
                    {"name": "StaticDuplicate", "payload": null, "wire_name": "static"}
                ],
                "RoleUnion": [
                    {"name": "RoleA", "payload": "RoleA"},
                    {"name": "RoleB", "payload": "RoleB"}
                ]
            },
            "aliases": {},
            "operations": {},
            "symbol_paths": {
                "Request": "crate::generated::types::Request",
                "RoleA": "crate::generated::types::RoleA",
                "RoleB": "crate::generated::types::RoleB",
                "RoleUnion": "crate::generated::types::RoleUnion"
            },
            "binding": {
                "client": {
                    "type_path": "crate::generated::Client",
                    "constructor": "new",
                    "api_key_builder": "with_api_key",
                    "base_url_builder": "with_base_url"
                },
                "type_preludes": []
            }
        }))
        .expect("canonical binding fixture");
        (index, bindings)
    }

    #[test]
    fn proves_nullable_array_of_scalar_reference_union() {
        let (index, bindings) = fixture();
        let schema = index.schema("Request").expect("Request schema");
        assert!(object_value_matches(&index, schema, "Request", &bindings));
    }

    #[test]
    fn rejects_scalar_union_collection_wire_drift() {
        let (index, mut bindings) = fixture();
        bindings.enums.get_mut("RoleB").expect("RoleB")[0].wire_name = Some("other".into());
        let schema = index.schema("Request").expect("Request schema");
        assert!(!object_value_matches(&index, schema, "Request", &bindings));
    }
}

#[cfg(test)]
mod transparent_box_tests {
    use super::*;
    use crate::contracts::OpenApi;

    fn fixture() -> (OpenApiIndex, Bindings) {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "components": {
                "schemas": {
                    "Group": {
                        "type": "object",
                        "properties": {"name": {"type": "string"}},
                        "required": ["name"]
                    },
                    "Condition": {
                        "type": "object",
                        "properties": {"ready": {"type": "boolean"}},
                        "required": ["ready"]
                    }
                }
            }
        }));
        let index = OpenApiIndex::new(&openapi).expect("OpenAPI index");
        let bindings = serde_json::from_value(serde_json::json!({
            "schema_version": 3,
            "structs": {
                "Group": [{"name": "name", "type": "String"}],
                "Condition": [{"name": "ready", "type": "bool"}]
            },
            "enums": {
                "RecursiveUnion": [
                    {"name": "Group", "payload": "Box<Group>"},
                    {"name": "Condition", "payload": "Condition"}
                ]
            },
            "aliases": {},
            "operations": {},
            "symbol_paths": {
                "Group": "crate::generated::types::Group",
                "Condition": "crate::generated::types::Condition",
                "RecursiveUnion": "crate::generated::types::RecursiveUnion"
            },
            "binding": {
                "client": {
                    "type_path": "crate::generated::Client",
                    "constructor": "new",
                    "api_key_builder": "with_api_key",
                    "base_url_builder": "with_base_url"
                },
                "type_preludes": []
            }
        }))
        .expect("boxed binding fixture");
        (index, bindings)
    }

    #[test]
    fn box_is_transparent_for_referenced_wire_identity() {
        let (_, bindings) = fixture();
        let schema = serde_json::json!({"$ref": "#/components/schemas/Group"});
        assert!(rust_type_matches_schema(&schema, "Box<Group>", &bindings));
    }

    #[test]
    fn boxed_recursive_union_payload_matches_referenced_branch() {
        let (index, bindings) = fixture();
        let schema = serde_json::json!({
            "anyOf": [
                {"$ref": "#/components/schemas/Group"},
                {"$ref": "#/components/schemas/Condition"}
            ]
        });
        assert!(request_union_matches(
            &index,
            &schema,
            "RecursiveUnion",
            &bindings
        ));
    }

    #[test]
    fn boxed_recursive_union_still_requires_bijective_payloads() {
        let (index, mut bindings) = fixture();
        bindings.enums.get_mut("RecursiveUnion").expect("union")[1].payload =
            Some("Box<Group>".into());
        let schema = serde_json::json!({
            "anyOf": [
                {"$ref": "#/components/schemas/Group"},
                {"$ref": "#/components/schemas/Condition"}
            ]
        });
        assert!(!request_union_matches(
            &index,
            &schema,
            "RecursiveUnion",
            &bindings
        ));
    }
}

#[cfg(test)]
mod referenced_array_union_tests {
    use super::*;
    use crate::contracts::OpenApi;

    fn fixture() -> (OpenApiIndex, Bindings, Value) {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "components": {"schemas": {
                "WireGroup": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}},
                    "required": ["name"]
                },
                "WireOther": {
                    "type": "object",
                    "properties": {"ready": {"type": "boolean"}},
                    "required": ["ready"]
                }
            }}
        }));
        let index = OpenApiIndex::new(&openapi).expect("index");
        let bindings = serde_json::from_value(serde_json::json!({
            "schema_version": 3,
            "structs": {
                "OpaqueGroup": [{"name": "name", "type": "String"}],
                "OpaqueOther": [{"name": "ready", "type": "bool"}],
                "OpaqueMap": [{
                    "name": "additional_properties",
                    "type": "std::collections::BTreeMap<String, serde_json::Value>"
                }]
            },
            "enums": {
                "OpaqueUnion": [
                    {"name": "Group", "payload": "GroupArray"},
                    {"name": "Other", "payload": "OtherArray"},
                    {"name": "Map", "payload": "MapArray"}
                ]
            },
            "aliases": {
                "GroupArray": "Vec<OpaqueGroup>",
                "OtherArray": "Vec<OpaqueOther>",
                "MapArray": "Vec<OpaqueMap>"
            },
            "operations": {},
            "symbol_paths": {
                "OpaqueGroup": "crate::raw::OpaqueGroup",
                "OpaqueOther": "crate::raw::OpaqueOther",
                "OpaqueMap": "crate::raw::OpaqueMap",
                "OpaqueUnion": "crate::raw::OpaqueUnion",
                "GroupArray": "crate::raw::GroupArray",
                "OtherArray": "crate::raw::OtherArray",
                "MapArray": "crate::raw::MapArray"
            },
            "binding": {"client": {
                "type_path": "crate::raw::Client",
                "constructor": "new",
                "api_key_builder": "with_api_key",
                "base_url_builder": "with_base_url"
            }, "type_preludes": []}
        }))
        .expect("bindings");
        let response = serde_json::json!({
            "anyOf": [
                {"type": "array", "items": {"$ref": "#/components/schemas/WireGroup"}},
                {"type": "array", "items": {"$ref": "#/components/schemas/WireOther"}},
                {"type": "array", "items": {"type": "object", "additionalProperties": true}}
            ],
            "title": "Array union"
        });
        (index, bindings, response)
    }

    #[test]
    fn proves_renamed_reference_and_canonical_raw_map_array_bijectively() {
        let (openapi, bindings, schema) = fixture();
        assert!(response_array_union_matches(
            &openapi,
            &schema,
            "OpaqueUnion",
            &bindings
        ));
    }

    #[test]
    fn rejects_ambiguous_or_drifted_array_payloads_and_extra_constraints() {
        let (openapi, bindings, mut schema) = fixture();
        schema["anyOf"][1]["items"] = schema["anyOf"][0]["items"].clone();
        assert!(!response_array_union_matches(
            &openapi,
            &schema,
            "OpaqueUnion",
            &bindings
        ));
        let (openapi, mut bindings, schema) = fixture();
        bindings.structs.get_mut("OpaqueGroup").expect("group")[0].type_name = "bool".into();
        assert!(!response_array_union_matches(
            &openapi,
            &schema,
            "OpaqueUnion",
            &bindings
        ));
        let (openapi, bindings, mut schema) = fixture();
        schema["anyOf"][0]["minItems"] = serde_json::json!(1);
        assert!(!response_array_union_matches(
            &openapi,
            &schema,
            "OpaqueUnion",
            &bindings
        ));
    }
}

#[cfg(test)]
mod legacy_nullable_request_tests {
    use super::*;
    use crate::contracts::OpenApi;

    fn fixture() -> (OpenApiIndex, Bindings, Value) {
        let source = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "components": {
                "schemas": {
                    "Scope": {
                        "type": "string",
                        "enum": ["private", "workspace"]
                    }
                }
            }
        }));
        let index = OpenApiIndex::new(&source).expect("OpenAPI index");
        let bindings = serde_json::from_value(serde_json::json!({
            "schema_version": 3,
            "structs": {
                "RawPatch": [
                    {"name": "description", "type": "Option<Option<String>>", "wire_name": "description"},
                    {"name": "sharing_scope", "type": "Option<Option<Scope>>", "wire_name": "sharingScope"}
                ]
            },
            "enums": {
                "Scope": [
                    {"name": "Private", "wire_name": "private"},
                    {"name": "Workspace", "wire_name": "workspace"}
                ]
            },
            "aliases": {},
            "operations": {},
            "symbol_paths": {},
            "binding": {
                "client": {
                    "type_path": "crate::raw::Client",
                    "constructor": "new",
                    "api_key_builder": "with_api_key",
                    "base_url_builder": "with_base_url"
                },
                "type_preludes": []
            }
        }))
        .expect("canonical bindings");
        let schema = serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "description": {"type": "string", "nullable": true},
                "sharingScope": {
                    "$ref": "#/components/schemas/Scope",
                    "nullable": true
                }
            }
        });
        (index, bindings, schema)
    }

    #[test]
    fn proves_optional_legacy_nullable_scalar_and_renamed_reference_fields() {
        let (index, bindings, schema) = fixture();
        assert!(object_value_matches(&index, &schema, "RawPatch", &bindings));
    }

    #[test]
    fn rejects_wrong_option_depth_and_unsupported_nullable_union() {
        let (index, mut bindings, mut schema) = fixture();
        bindings.structs.get_mut("RawPatch").expect("raw patch")[0].type_name =
            "Option<String>".into();
        assert!(!object_value_matches(
            &index, &schema, "RawPatch", &bindings
        ));

        let (index, bindings, _) = fixture();
        schema["properties"]["sharingScope"]["maxLength"] = serde_json::json!(3);
        assert!(legacy_nullable_request_property(&schema["properties"]["sharingScope"]).is_none());
        assert!(!object_value_matches(
            &index, &schema, "RawPatch", &bindings
        ));

        let (index, bindings, _) = fixture();
        schema["properties"]["description"]["anyOf"] =
            serde_json::json!([{"type": "string"}, {"type": "integer"}]);
        assert!(legacy_nullable_request_property(&schema["properties"]["description"]).is_none());
        assert!(!object_value_matches(
            &index, &schema, "RawPatch", &bindings
        ));
    }
}
