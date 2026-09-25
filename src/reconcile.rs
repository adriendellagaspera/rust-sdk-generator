use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::contracts::{
    Bindings, OpenApi, OperationBinding, OperationBindingKind, OperationMetadataBinding,
    RequestMediaDefinition, ResponseRepresentationBinding,
};
use crate::error::{GenerationError, Result};
use crate::openapi::{OpenApiIndex, ref_name};
use crate::rust_type::parse_type;
use crate::structural::{
    ScalarFieldShape, inline_object_union_mapping, object_field_names_match, object_value_matches,
    raw_scalar_struct_shape, request_object_matches_with_discriminators,
    response_array_union_matches, rust_type_matches_schema, scalar_object_shape,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationMatch {
    pub binding: Option<String>,
    pub candidates: Vec<String>,
    pub reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestShape {
    parameters: BTreeSet<String>,
    body: Option<RequestBodyShape>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestBodyShape {
    Model(RequestMediaDefinition, String),
    OptionalNullableModel(RequestMediaDefinition, String),
    InlineModel(RequestMediaDefinition, Value),
    Schema(RequestMediaDefinition, Value),
    Raw(RequestMediaDefinition, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResponseShape {
    Empty,
    JsonRef(String),
    JsonUnion(BTreeSet<String>),
    JsonInlineObjectUnion(Value),
    JsonRecursiveUnion(Value),
    JsonObject(BTreeMap<String, ScalarFieldShape>),
    JsonArray(Value),
    Binary,
}

fn error(code: &'static str, message: impl Into<String>) -> GenerationError {
    GenerationError::new(code, message)
}

fn normalized_operations(openapi: &OpenApi) -> Result<BTreeMap<String, Value>> {
    OpenApiIndex::new(openapi)?;
    let root = openapi
        .0
        .as_object()
        .ok_or_else(|| error("openapi.invalid", "OpenAPI document must be a JSON object"))?;
    let mut operations = BTreeMap::new();
    let Some(paths) = root.get("paths").and_then(Value::as_object) else {
        return Ok(operations);
    };
    for path_item in paths.values() {
        let Some(path_item) = path_item.as_object() else {
            continue;
        };
        let path_parameters = path_item
            .get("parameters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for method in [
            "get", "put", "post", "delete", "patch", "head", "options", "trace",
        ] {
            let Some(operation) = path_item.get(method).and_then(Value::as_object) else {
                continue;
            };
            let Some(operation_id) = operation.get("operationId").and_then(Value::as_str) else {
                continue;
            };
            let mut normalized = operation.clone();
            let mut parameters = path_parameters.clone();
            parameters.extend(
                operation
                    .get("parameters")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
            normalized.insert("parameters".into(), Value::Array(parameters));
            operations.insert(operation_id.into(), Value::Object(normalized));
        }
    }
    Ok(operations)
}

fn normalized_parameter_name(value: &str) -> String {
    value.replace('-', "_")
}

fn optional_nullable_ref_request(schema: &Value) -> Option<&str> {
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
                | "$comment"
        )
    }) {
        return None;
    }
    let branches = object.get("anyOf")?.as_array()?;
    if branches.len() != 2 {
        return None;
    }
    let mut reference = None;
    let mut nulls = 0;
    for branch in branches {
        let branch = branch.as_object()?;
        if branch.get("type").and_then(Value::as_str) == Some("null") {
            if branch.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "type"
                        | "title"
                        | "description"
                        | "deprecated"
                        | "example"
                        | "examples"
                        | "default"
                        | "$comment"
                )
            }) {
                return None;
            }
            nulls += 1;
            continue;
        }
        let target = branch.get("$ref").and_then(Value::as_str)?;
        let target = target.strip_prefix("#/components/schemas/")?;
        if target.is_empty()
            || branch.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "$ref"
                        | "title"
                        | "description"
                        | "deprecated"
                        | "example"
                        | "examples"
                        | "default"
                        | "$comment"
                )
            })
            || reference.replace(target).is_some()
        {
            return None;
        }
    }
    if nulls == 1 { reference } else { None }
}

fn request_shape(operation: &Value) -> std::result::Result<RequestShape, &'static str> {
    let mut parameters = BTreeSet::new();
    for parameter in operation
        .get("parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(parameter) = parameter.as_object() else {
            return Err("request.parameter_binding_unsupported");
        };
        if parameter.contains_key("$ref") {
            return Err("request.parameter_binding_unsupported");
        }
        if !matches!(
            parameter.get("in").and_then(Value::as_str),
            Some("path") | Some("query") | Some("header")
        ) {
            return Err("request.parameter_binding_unsupported");
        }
        let Some(name) = parameter.get("name").and_then(Value::as_str) else {
            return Err("request.parameter_binding_unsupported");
        };
        if !parameters.insert(normalized_parameter_name(name)) {
            return Err("request.parameter_binding_unsupported");
        }
    }

    let request_body = operation.get("requestBody");
    let required = request_body
        .and_then(|body| body.get("required"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let body = match request_body
        .and_then(|body| body.get("content"))
        .and_then(Value::as_object)
    {
        None => None,
        Some(content) if content.is_empty() => None,
        Some(content) if content.len() == 1 => {
            let (media_type, payload) = content.iter().next().expect("one request media");
            let schema = payload
                .get("schema")
                .ok_or("request.inline_or_unresolved")?;
            match media_type.as_str() {
                "application/json"
                | "multipart/form-data"
                | "application/x-www-form-urlencoded" => {
                    let media = match media_type.as_str() {
                        "application/json" => RequestMediaDefinition::Json,
                        "multipart/form-data" => RequestMediaDefinition::MultipartFormData,
                        "application/x-www-form-urlencoded" => {
                            RequestMediaDefinition::FormUrlencoded
                        }
                        _ => unreachable!(),
                    };
                    if let Some(schema) = ref_name(schema) {
                        Some(RequestBodyShape::Model(media, schema.to_owned()))
                    } else if media == RequestMediaDefinition::Json
                        && !required
                        && let Some(schema) = optional_nullable_ref_request(schema)
                    {
                        Some(RequestBodyShape::OptionalNullableModel(
                            media,
                            schema.to_owned(),
                        ))
                    } else if schema.get("type").and_then(Value::as_str) == Some("object") {
                        Some(RequestBodyShape::InlineModel(media, schema.clone()))
                    } else if media == RequestMediaDefinition::Json && required {
                        Some(RequestBodyShape::Schema(media, schema.clone()))
                    } else {
                        return Err("request.inline_or_unresolved");
                    }
                }
                "application/octet-stream"
                    if schema.get("type").and_then(Value::as_str) == Some("string")
                        && schema.get("format").and_then(Value::as_str) == Some("binary") =>
                {
                    let raw = if required {
                        "Vec<u8>".to_owned()
                    } else {
                        "Option<Vec<u8>>".to_owned()
                    };
                    Some(RequestBodyShape::Raw(
                        RequestMediaDefinition::OctetStream,
                        raw,
                    ))
                }
                "text/plain"
                    if schema.get("type").and_then(Value::as_str) == Some("string")
                        && schema.get("format").is_none() =>
                {
                    let raw = if required {
                        "String".to_owned()
                    } else {
                        "Option<String>".to_owned()
                    };
                    Some(RequestBodyShape::Raw(
                        RequestMediaDefinition::TextPlain,
                        raw,
                    ))
                }
                _ if schema.get("type").and_then(Value::as_str) == Some("string")
                    && schema.get("format").and_then(Value::as_str) == Some("binary") =>
                {
                    let raw = if required {
                        "Vec<u8>".to_owned()
                    } else {
                        "Option<Vec<u8>>".to_owned()
                    };
                    Some(RequestBodyShape::Raw(RequestMediaDefinition::Binary, raw))
                }
                _ => return Err("request.media_projection_unsupported"),
            }
        }
        Some(_) => return Err("request.media_projection_unsupported"),
    };

    Ok(RequestShape { parameters, body })
}

fn successful_responses(operation: &Value) -> Vec<(&String, &Value)> {
    operation
        .get("responses")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(Map::iter)
        .filter(|(status, _)| status.starts_with('2'))
        .collect()
}

fn response_shape(operation: &Value) -> std::result::Result<ResponseShape, &'static str> {
    let success = successful_responses(operation);
    if success.len() != 1 {
        return Err("response.multiple_success_contracts");
    }
    let content = success[0].1.get("content").and_then(Value::as_object);
    let Some(content) = content.filter(|content| !content.is_empty()) else {
        return Ok(ResponseShape::Empty);
    };
    if content.len() != 1 {
        return Err("transport.source_operation_identity_required");
    }
    let (media, payload) = content.iter().next().expect("one response media");
    let Some(schema) = payload.get("schema") else {
        return Err("response.inline_or_unresolved");
    };
    if media == "application/json" {
        if let Some(reference) = ref_name(schema) {
            return Ok(ResponseShape::JsonRef(reference.into()));
        }
        let branches = schema
            .get("oneOf")
            .or_else(|| schema.get("anyOf"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let references: BTreeSet<_> = branches
            .iter()
            .filter_map(ref_name)
            .map(str::to_owned)
            .collect();
        if branches.len() >= 2 && references.len() == branches.len() {
            return Ok(ResponseShape::JsonUnion(references));
        }
        if branches.len() >= 2
            && references.is_empty()
            && branches
                .iter()
                .all(|branch| scalar_object_shape(branch).is_some())
        {
            return Ok(ResponseShape::JsonInlineObjectUnion(schema.clone()));
        }
        if branches.len() >= 2 {
            // Non-object unions are eligible for reconciliation only when
            // the complete recursive Rust payload mapping proves the wire shape.
            return Ok(ResponseShape::JsonRecursiveUnion(schema.clone()));
        }
        if let Some(fields) = scalar_object_shape(schema) {
            return Ok(ResponseShape::JsonObject(fields));
        }
        if schema.get("type").and_then(Value::as_str) == Some("array") {
            return Ok(ResponseShape::JsonArray(schema.clone()));
        }
        return Err("response.inline_or_unresolved");
    }
    if schema.get("type").and_then(Value::as_str) == Some("string")
        && schema.get("format").and_then(Value::as_str) == Some("binary")
    {
        return Ok(ResponseShape::Binary);
    }
    Err("response.non_json_success")
}

fn selected_success_responses<'a>(
    operation: &'a Value,
    statuses: &[String],
) -> Option<Vec<(&'a String, &'a Value)>> {
    let success = successful_responses(operation);
    if statuses.is_empty() {
        return (!success.is_empty()).then_some(success);
    }
    let expected: BTreeSet<_> = statuses.iter().map(String::as_str).collect();
    if expected.len() != statuses.len() {
        return None;
    }
    let selected: Vec<_> = success
        .into_iter()
        .filter(|(status, _)| expected.contains(status.as_str()))
        .collect();
    (selected.len() == expected.len()).then_some(selected)
}

fn response_payload<'a>(response: &'a Value, media_type: &str) -> Option<&'a Value> {
    response
        .get("content")
        .and_then(Value::as_object)?
        .get(media_type)?
        .get("schema")
}

fn binary_schema(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("string")
        && schema.get("format").and_then(Value::as_str) == Some("binary")
}

pub(crate) fn unconstrained_json_alias_matches(
    schema: &Value,
    raw_success: &str,
    bindings: &Bindings,
) -> bool {
    schema.as_object().is_some_and(Map::is_empty)
        && bindings
            .aliases
            .get(raw_success)
            .is_some_and(|alias| alias == "serde_json::Value")
}

fn canonical_json_schema_matches(
    openapi: &OpenApiIndex,
    schema: &Value,
    schema_name: &str,
    binding: &OperationBinding,
    bindings: &Bindings,
) -> bool {
    if let Some(reference) = ref_name(schema) {
        return reference == schema_name;
    }
    if unconstrained_json_alias_matches(schema, &binding.success_type, bindings) {
        return true;
    }

    let branches = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array);
    if let Some(branches) = branches {
        let references = branches
            .iter()
            .filter_map(ref_name)
            .collect::<BTreeSet<_>>();
        if references.len() == branches.len()
            && let Some(variants) = bindings.enums.get(&binding.success_type)
        {
            let actual = variants
                .iter()
                .filter_map(|variant| variant.payload.as_deref())
                .collect::<BTreeSet<_>>();
            if actual == references && variants.iter().all(|variant| variant.payload.is_some()) {
                return true;
            }
        }
        if inline_object_union_mapping(schema, &binding.success_type, bindings).is_some() {
            return true;
        }
    }

    if let Some(wire) = scalar_object_shape(schema) {
        return raw_scalar_struct_shape(bindings, &binding.success_type)
            .is_some_and(|actual| actual == wire);
    }
    if schema.get("type").and_then(Value::as_str) == Some("object")
        && object_field_names_match(schema, &binding.success_type, bindings)
    {
        return true;
    }
    rust_type_matches_schema(schema, &binding.success_type, bindings)
        || response_array_union_matches(openapi, schema, &binding.success_type, bindings)
}

fn metadata_response_matches(
    openapi: &OpenApiIndex,
    operation: &Value,
    binding: &OperationBinding,
    metadata: &OperationMetadataBinding,
    bindings: &Bindings,
) -> bool {
    let Some(successes) = selected_success_responses(operation, &metadata.success_statuses) else {
        return false;
    };
    match &metadata.representation {
        ResponseRepresentationBinding::Empty => {
            binding.success_type == "()"
                && successes.iter().all(|(_, response)| {
                    response
                        .get("content")
                        .and_then(Value::as_object)
                        .is_none_or(|content| content.is_empty())
                })
        }
        ResponseRepresentationBinding::Json {
            schema_name,
            media_type,
        } => {
            binding.success_type == *schema_name
                && successes.iter().all(|(_, response)| {
                    response_payload(response, media_type).is_some_and(|schema| {
                        canonical_json_schema_matches(
                            openapi,
                            schema,
                            schema_name,
                            binding,
                            bindings,
                        )
                    })
                })
        }
        ResponseRepresentationBinding::Text { media_type } => {
            binding.success_type == "String"
                && successes.iter().all(|(_, response)| {
                    response_payload(response, media_type).is_some_and(|schema| {
                        schema.get("type").and_then(Value::as_str) == Some("string")
                            && schema.get("format").is_none()
                    })
                })
        }
        ResponseRepresentationBinding::BinaryBuffered { media_type, .. } => {
            matches!(binding.success_type.as_str(), "bytes::Bytes" | "Vec<u8>")
                && binding.stream.is_none()
                && successes.iter().all(|(_, response)| {
                    response_payload(response, media_type).is_some_and(binary_schema)
                })
        }
        ResponseRepresentationBinding::EventStream { media_type } => {
            binding.stream.is_some()
                && successes
                    .iter()
                    .all(|(_, response)| response_payload(response, media_type).is_some())
        }
        ResponseRepresentationBinding::BinaryStream { media_type, .. } => {
            binding.stream.is_some()
                && successes.iter().all(|(_, response)| {
                    response_payload(response, media_type).is_some_and(binary_schema)
                })
        }
    }
}

fn raw_parameter_name(value: &str) -> &str {
    value.strip_prefix("r#").unwrap_or(value)
}

fn response_matches(
    shape: &ResponseShape,
    binding: &OperationBinding,
    bindings: &Bindings,
) -> bool {
    match shape {
        ResponseShape::Empty => binding.success_type == "()",
        ResponseShape::JsonRef(reference) => binding.success_type == *reference,
        ResponseShape::JsonUnion(references) => bindings
            .enums
            .get(&binding.success_type)
            .map(|variants| {
                variants
                    .iter()
                    .filter_map(|variant| variant.payload.clone())
                    .collect::<BTreeSet<_>>()
                    == *references
                    && variants.iter().all(|variant| variant.payload.is_some())
            })
            .unwrap_or(false),
        ResponseShape::JsonInlineObjectUnion(schema) => {
            inline_object_union_mapping(schema, &binding.success_type, bindings).is_some()
                || rust_type_matches_schema(schema, &binding.success_type, bindings)
        }
        ResponseShape::JsonRecursiveUnion(schema) => {
            bindings.enums.contains_key(&binding.success_type)
                && rust_type_matches_schema(schema, &binding.success_type, bindings)
        }
        ResponseShape::JsonObject(fields) => {
            raw_scalar_struct_shape(bindings, &binding.success_type)
                .is_some_and(|raw| raw == *fields)
        }
        ResponseShape::JsonArray(schema) => {
            bindings.aliases.contains_key(&binding.success_type)
                && rust_type_matches_schema(schema, &binding.success_type, bindings)
        }
        ResponseShape::Binary => {
            let Ok(success) = parse_type(&binding.success_type) else {
                return false;
            };
            let Some(stream) = &binding.stream else {
                return false;
            };
            success.constructor.as_deref() == Some("futures_util::stream::BoxStream")
                && success.arguments.len() == 2
                && stream.item_type == "bytes::Bytes"
                && stream.lifetime == "'static"
        }
    }
}

fn canonical_source_identity_matches(
    operation_id: &str,
    operation: &Value,
    binding: &OperationBinding,
) -> bool {
    let Some(metadata) = &binding.metadata else {
        return true;
    };
    if metadata.kind != OperationBindingKind::CallShape {
        return false;
    }
    let Some(method) = operation.get("x-sdk-method").and_then(Value::as_str) else {
        return false;
    };
    let Some(path) = operation.get("x-sdk-path").and_then(Value::as_str) else {
        return false;
    };
    metadata.source_operation.operation_id == operation_id
        && metadata
            .source_operation
            .method
            .eq_ignore_ascii_case(method)
        && metadata.source_operation.path == path
}

fn has_source_operation_id(bindings: &Bindings, operation_id: &str) -> bool {
    bindings.operations.values().any(|binding| {
        binding.metadata.as_ref().is_some_and(|metadata| {
            metadata.kind == OperationBindingKind::CallShape
                && metadata.source_operation.operation_id == operation_id
        })
    })
}

fn binding_matches(
    openapi: &OpenApiIndex,
    operation: &Value,
    request: &RequestShape,
    response: Option<&ResponseShape>,
    binding: &OperationBinding,
    bindings: &Bindings,
) -> bool {
    if binding
        .metadata
        .as_ref()
        .is_some_and(|metadata| metadata.request_discriminator_unproven)
    {
        return false;
    }
    let mut body_index = None;
    if let Some(body) = &request.body {
        let matching: Vec<_> = binding
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, parameter)| match body {
                RequestBodyShape::Model(_, schema) => {
                    let discriminators = binding
                        .metadata
                        .as_ref()
                        .map(|metadata| metadata.request_discriminators.as_slice())
                        .unwrap_or_default();
                    parameter.type_name == *schema
                        || request_object_matches_with_discriminators(
                            openapi,
                            schema,
                            &parameter.type_name,
                            bindings,
                            discriminators,
                        )
                }
                RequestBodyShape::OptionalNullableModel(_, schema) => {
                    let Ok(raw) = parse_type(&parameter.type_name) else {
                        return false;
                    };
                    let Some(present) = raw.unary("Option") else {
                        return false;
                    };
                    let Some(nullable) = present.unary("Option") else {
                        return false;
                    };
                    if nullable.unary("Option").is_some() {
                        return false;
                    }
                    let discriminators = binding
                        .metadata
                        .as_ref()
                        .map(|metadata| metadata.request_discriminators.as_slice())
                        .unwrap_or_default();
                    nullable.spelling == *schema
                        || request_object_matches_with_discriminators(
                            openapi,
                            schema,
                            &nullable.spelling,
                            bindings,
                            discriminators,
                        )
                }
                RequestBodyShape::InlineModel(_, schema) => {
                    object_value_matches(openapi, schema, &parameter.type_name, bindings)
                }
                RequestBodyShape::Schema(_, schema) => {
                    rust_type_matches_schema(schema, &parameter.type_name, bindings)
                }
                RequestBodyShape::Raw(_, type_name) => parameter.type_name == *type_name,
            })
            .map(|(index, _)| index)
            .collect();
        if matching.len() != 1 {
            return false;
        }
        body_index = Some(matching[0]);
    }

    let raw_names: Vec<_> = binding
        .parameters
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != body_index)
        .map(|(_, parameter)| raw_parameter_name(&parameter.name).to_owned())
        .collect();
    if let Some(metadata) = &binding.metadata
        && !metadata.parameter_wires.is_empty()
    {
        // Every non-path source parameter must have a unique *observed*
        // HTTP location and wire key. A normalized Rust spelling alone cannot
        // distinguish dotted query names, casing or query/header collisions.
        let mut expected = BTreeSet::new();
        let mut fallback_names = Vec::new();
        for parameter in operation
            .get("parameters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(location), Some(name)) = (
                parameter.get("in").and_then(Value::as_str),
                parameter.get("name").and_then(Value::as_str),
            ) else {
                return false;
            };
            match location {
                "path" => fallback_names.push(normalized_parameter_name(name)),
                "query" | "header" => {
                    let wire = if location == "header" {
                        name.to_ascii_lowercase()
                    } else {
                        name.to_owned()
                    };
                    if !expected.insert((location.to_owned(), wire)) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        let mut matched = BTreeSet::new();
        let mut mapped_rust = BTreeSet::new();
        for wire in &metadata.parameter_wires {
            let name = raw_parameter_name(&wire.rust_name);
            if !raw_names.iter().any(|raw| raw == name) || !mapped_rust.insert(name.to_owned()) {
                return false;
            }
            let key = if wire.location == "header" {
                wire.wire_name.to_ascii_lowercase()
            } else {
                wire.wire_name.clone()
            };
            let identity = (wire.location.clone(), key);
            if !expected.contains(&identity) || !matched.insert(identity) {
                return false;
            }
        }

        // Exact observed wire evidence is authoritative. Source query/header
        // parameters not covered by that evidence may fall back to normalized
        // Rust names only when the remainder is a strict bijection. This keeps
        // ordinary parameters usable without guessing any missing collision.
        for (location, wire_name) in expected.difference(&matched) {
            let source_name = operation
                .get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find_map(|parameter| {
                    let candidate_location = parameter.get("in").and_then(Value::as_str)?;
                    let candidate_name = parameter.get("name").and_then(Value::as_str)?;
                    let candidate_wire = if candidate_location == "header" {
                        candidate_name.to_ascii_lowercase()
                    } else {
                        candidate_name.to_owned()
                    };
                    (candidate_location == location.as_str()
                        && candidate_wire == wire_name.as_str())
                    .then_some(candidate_name)
                })
                .expect("expected wire identity came from this operation");
            fallback_names.push(normalized_parameter_name(source_name));
        }
        let expected_fallback: BTreeSet<_> = fallback_names.iter().cloned().collect();
        if expected_fallback.len() != fallback_names.len() {
            return false;
        }
        let raw_fallback: Vec<_> = raw_names
            .iter()
            .filter(|name| !mapped_rust.contains(name.as_str()))
            .cloned()
            .collect();
        let actual_fallback: BTreeSet<_> = raw_fallback.iter().cloned().collect();
        if actual_fallback.len() != raw_fallback.len() || actual_fallback != expected_fallback {
            return false;
        }
    } else {
        let raw_set: BTreeSet<_> = raw_names.iter().cloned().collect();
        if raw_names.len() != raw_set.len() || raw_set != request.parameters {
            return false;
        }
    }
    if let Some(metadata) = &binding.metadata {
        metadata_response_matches(openapi, operation, binding, metadata, bindings)
    } else {
        response.is_some_and(|shape| response_matches(shape, binding, bindings))
    }
}

pub(crate) fn reconcile(
    openapi: &OpenApi,
    bindings: &Bindings,
) -> Result<BTreeMap<String, OperationMatch>> {
    let operations = normalized_operations(openapi)?;
    let index = OpenApiIndex::new(openapi)?;
    let mut candidates: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut reasons = BTreeMap::new();

    for (operation_id, operation) in &operations {
        let request = match request_shape(operation) {
            Ok(shape) => shape,
            Err(reason) => {
                reasons.insert(operation_id.clone(), reason);
                continue;
            }
        };
        let response = if bindings.schema_version == 3 {
            None
        } else {
            match response_shape(operation) {
                Ok(shape) => Some(shape),
                Err(reason) => {
                    reasons.insert(operation_id.clone(), reason);
                    continue;
                }
            }
        };
        let canonical_operation = index.operation(operation_id)?;
        let source_candidates: Vec<_> = bindings
            .operations
            .iter()
            .filter(|(_, binding)| {
                canonical_source_identity_matches(operation_id, canonical_operation, binding)
            })
            .collect();
        if bindings.schema_version == 3
            && has_source_operation_id(bindings, operation_id)
            && source_candidates.is_empty()
        {
            reasons.insert(
                operation_id.clone(),
                "bindings.source_operation_identity_mismatch",
            );
            continue;
        }
        candidates.insert(
            operation_id.clone(),
            source_candidates
                .into_iter()
                .filter(|(_, binding)| {
                    binding_matches(
                        &index,
                        operation,
                        &request,
                        response.as_ref(),
                        binding,
                        bindings,
                    )
                })
                .map(|(name, _)| name.clone())
                .collect(),
        );
    }

    let mut reverse: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (operation_id, matches) in &candidates {
        for candidate in matches {
            reverse
                .entry(candidate.clone())
                .or_default()
                .insert(operation_id.clone());
        }
    }

    let mut result = BTreeMap::new();
    for operation_id in operations.keys() {
        if let Some(reason) = reasons.get(operation_id) {
            result.insert(
                operation_id.clone(),
                OperationMatch {
                    binding: None,
                    candidates: Vec::new(),
                    reason: Some(*reason),
                },
            );
            continue;
        }
        let matches = &candidates[operation_id];
        let outcome = if matches.is_empty() {
            OperationMatch {
                binding: None,
                candidates: Vec::new(),
                reason: Some("bindings.no_structural_match"),
            }
        } else if matches.len() != 1
            || reverse
                .get(&matches[0])
                .is_some_and(|operations| operations.len() != 1)
        {
            OperationMatch {
                binding: None,
                candidates: matches.clone(),
                reason: Some("bindings.source_operation_identity_required"),
            }
        } else {
            OperationMatch {
                binding: Some(matches[0].clone()),
                candidates: matches.clone(),
                reason: None,
            }
        };
        result.insert(operation_id.clone(), outcome);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{
        BindingLayout, ClientBinding, OperationBindingKind, OperationMetadataBinding,
        ParameterBinding, ParameterWireBinding, ResponseRepresentationBinding,
        SourceOperationBinding, StreamBinding,
    };

    fn bindings(operations: BTreeMap<String, OperationBinding>) -> Bindings {
        Bindings {
            schema_version: 2,
            structs: BTreeMap::new(),
            enums: BTreeMap::new(),
            aliases: BTreeMap::new(),
            operations,
            symbol_paths: BTreeMap::new(),
            binding: BindingLayout {
                client: ClientBinding {
                    type_path: "crate::raw::Client".into(),
                    constructor: "new".into(),
                    api_key_builder: "with_api_key".into(),
                    base_url_builder: "with_base_url".into(),
                },
                type_preludes: Vec::new(),
            },
        }
    }

    fn operation(parameters: Vec<ParameterBinding>, success_type: &str) -> OperationBinding {
        OperationBinding {
            name: "opaque".into(),
            parameters,
            return_type: format!("Result<{success_type}, Error>"),
            success_type: success_type.into(),
            stream: None,
            metadata: None,
        }
    }

    fn v3_bindings(operations: BTreeMap<String, OperationBinding>) -> Bindings {
        Bindings {
            schema_version: 3,
            structs: BTreeMap::new(),
            enums: BTreeMap::new(),
            aliases: BTreeMap::new(),
            operations,
            symbol_paths: BTreeMap::new(),
            binding: BindingLayout {
                client: ClientBinding {
                    type_path: "crate::raw::Client".into(),
                    constructor: "new".into(),
                    api_key_builder: "with_api_key".into(),
                    base_url_builder: "with_base_url".into(),
                },
                type_preludes: Vec::new(),
            },
        }
    }

    fn v3_operation_with_response(
        key: &str,
        operation_id: &str,
        method: &str,
        path: &str,
        kind: OperationBindingKind,
        response: (
            &str,
            ResponseRepresentationBinding,
            Vec<&str>,
            Option<StreamBinding>,
        ),
    ) -> OperationBinding {
        let (success_type, representation, success_statuses, stream) = response;
        OperationBinding {
            name: key.into(),
            parameters: Vec::new(),
            return_type: format!("Result<{success_type}, Error>"),
            success_type: success_type.into(),
            stream,
            metadata: Some(OperationMetadataBinding {
                kind,
                source_operation: SourceOperationBinding {
                    operation_id: operation_id.into(),
                    method: method.into(),
                    path: path.into(),
                },
                emitted_operation_id: operation_id.into(),
                representation,
                success_statuses: success_statuses.into_iter().map(str::to_owned).collect(),
                request_discriminators: Vec::new(),
                request_discriminator_unproven: false,
                parameter_wires: Vec::new(),
                stream_abi: None,
            }),
        }
    }

    fn v3_operation(
        key: &str,
        operation_id: &str,
        method: &str,
        path: &str,
        kind: OperationBindingKind,
    ) -> OperationBinding {
        v3_operation_with_response(
            key,
            operation_id,
            method,
            path,
            kind,
            (
                "()",
                ResponseRepresentationBinding::Empty,
                vec!["204"],
                None,
            ),
        )
    }

    #[test]
    fn canonical_wire_map_distinguishes_query_and_header_cursors() {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/events/{event_id}": {
                    "get": {
                        "operationId": "read_events",
                        "parameters": [
                            {"name": "event_id", "in": "path", "required": true,
                             "schema": {"type": "string"}},
                            {"name": "last_event_id", "in": "query", "required": false,
                             "schema": {"type": "string"}},
                            {"name": "Last-Event-ID", "in": "header", "required": false,
                             "schema": {"type": "string"}},
                            {"name": "fields", "in": "query", "required": false,
                             "schema": {"type": "array", "items": {"type": "string"}}}
                        ],
                        "responses": {"204": {"description": "done"}}
                    }
                }
            }
        }));
        let mut raw = v3_operation(
            "opaque_events",
            "read_events",
            "GET",
            "/events/{event_id}",
            OperationBindingKind::CallShape,
        );
        raw.parameters = vec![
            ParameterBinding {
                name: "last_event_id".into(),
                type_name: "Option<impl AsRef<str>>".into(),
            },
            ParameterBinding {
                name: "event_id".into(),
                type_name: "impl AsRef<str>".into(),
            },
            ParameterBinding {
                name: "last_event_id_2".into(),
                type_name: "Option<impl AsRef<str>>".into(),
            },
            ParameterBinding {
                name: "fields".into(),
                type_name: "Option<Vec<String>>".into(),
            },
        ];
        let proven = vec![
            ParameterWireBinding {
                rust_name: "last_event_id".into(),
                location: "query".into(),
                wire_name: "last_event_id".into(),
            },
            ParameterWireBinding {
                rust_name: "last_event_id_2".into(),
                location: "header".into(),
                wire_name: "Last-Event-ID".into(),
            },
        ];
        let mut check = |wires: Vec<ParameterWireBinding>, accepted: bool| {
            raw.metadata.as_mut().expect("v3 metadata").parameter_wires = wires;
            let result = reconcile(
                &openapi,
                &v3_bindings(BTreeMap::from([("opaque_events".into(), raw.clone())])),
            )
            .expect("candidate reconciliation");
            assert_eq!(
                result["read_events"].binding.as_deref() == Some("opaque_events"),
                accepted
            );
        };
        // The exact collision-prone cursor wires are proven from emitted AST,
        // while the ordinary `fields` query parameter and path id are completed
        // by the strict normalized-name bijection.
        check(proven.clone(), true);
        let mut swapped = proven.clone();
        swapped[0].rust_name = "last_event_id_2".into();
        swapped[1].rust_name = "last_event_id".into();
        // A swapped Rust-to-wire map cannot be independently disproved by
        // signature names alone; the producer must derive it from emitted AST.
        // The consumer checks exact source wire keys and bijectivity.
        check(swapped, true);
        let mut wrong_wire = proven.clone();
        wrong_wire[0].wire_name = "last-event-id".into();
        check(wrong_wire, false);
        let mut wrong_location = proven.clone();
        wrong_location[1].location = "query".into();
        check(wrong_location, false);
        check(proven[..1].to_vec(), false);
        check(vec![proven[0].clone(), proven[0].clone()], false);
        check(Vec::new(), false);
    }

    #[test]
    fn reconciles_required_array_union_and_exact_optional_nullable_root() {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/members": {"post": {
                    "operationId": "create_members",
                    "requestBody": {
                        "required": true,
                        "content": {"application/json": {"schema": {
                            "type": "array",
                            "items": {"$ref": "#/components/schemas/Member"}
                        }}}
                    },
                    "responses": {"204": {"description": "done"}}
                }},
                "/metrics": {"put": {
                    "operationId": "update_metrics",
                    "requestBody": {
                        "required": true,
                        "content": {"application/json": {"schema": {
                            "anyOf": [
                                {"$ref": "#/components/schemas/Online"},
                                {"$ref": "#/components/schemas/Offline"}
                            ]
                        }}}
                    },
                    "responses": {"204": {"description": "done"}}
                }},
                "/pause": {"post": {
                    "operationId": "pause",
                    "requestBody": {
                        "content": {"application/json": {"schema": {
                            "anyOf": [
                                {"$ref": "#/components/schemas/PauseRequest"},
                                {"type": "null"}
                            ]
                        }}}
                    },
                    "responses": {"204": {"description": "done"}}
                }}
            },
            "components": {"schemas": {
                "Member": {
                    "type": "object",
                    "required": ["name"],
                    "properties": {"name": {"type": "string"}}
                },
                "Online": {
                    "type": "object",
                    "required": ["online"],
                    "properties": {"online": {"type": "boolean"}}
                },
                "Offline": {
                    "type": "object",
                    "required": ["batch"],
                    "properties": {"batch": {"type": "string"}}
                },
                "PauseRequest": {
                    "type": "object",
                    "properties": {"reason": {"type": "string"}}
                }
            }}
        }));
        let bindings: Bindings = serde_json::from_value(serde_json::json!({
            "schema_version": 3,
            "structs": {
                "Member": [{"name": "name", "wire_name": "name", "type": "String"}],
                "Online": [{"name": "online", "wire_name": "online", "type": "bool"}],
                "Offline": [{"name": "batch", "wire_name": "batch", "type": "String"}],
                "PauseRequest": [{"name": "reason", "wire_name": "reason", "type": "Option<String>"}]
            },
            "enums": {
                "MetricsRequest": [
                    {"name": "Online", "payload": "Online", "wire_name": "online"},
                    {"name": "Offline", "payload": "Offline", "wire_name": "offline"}
                ]
            },
            "aliases": {
                "UsersRequest": "Vec<Member>",
                "PauseAlias": "PauseRequest"
            },
            "operations": {
                "raw_create_members": {
                    "name": "raw_create_members",
                    "parameters": [{"name": "request", "type": "UsersRequest"}],
                    "return_type": "Result<(), Error>",
                    "success_type": "()",
                    "stream": null,
                    "metadata": {
                        "kind": "call_shape",
                        "source_operation": {
                            "operation_id": "create_members",
                            "method": "POST",
                            "path": "/members"
                        },
                        "emitted_operation_id": "create_members",
                        "representation": {"kind": "empty"},
                        "success_statuses": ["204"],
                        "request_discriminators": [],
                        "stream_abi": null
                    }
                },
                "raw_update_metrics": {
                    "name": "raw_update_metrics",
                    "parameters": [{"name": "request", "type": "MetricsRequest"}],
                    "return_type": "Result<(), Error>",
                    "success_type": "()",
                    "stream": null,
                    "metadata": {
                        "kind": "call_shape",
                        "source_operation": {
                            "operation_id": "update_metrics",
                            "method": "PUT",
                            "path": "/metrics"
                        },
                        "emitted_operation_id": "update_metrics",
                        "representation": {"kind": "empty"},
                        "success_statuses": ["204"],
                        "request_discriminators": [],
                        "stream_abi": null
                    }
                },
                "raw_pause": {
                    "name": "raw_pause",
                    "parameters": [{"name": "request", "type": "Option<Option<PauseAlias>>"}],
                    "return_type": "Result<(), Error>",
                    "success_type": "()",
                    "stream": null,
                    "metadata": {
                        "kind": "call_shape",
                        "source_operation": {
                            "operation_id": "pause",
                            "method": "POST",
                            "path": "/pause"
                        },
                        "emitted_operation_id": "pause",
                        "representation": {"kind": "empty"},
                        "success_statuses": ["204"],
                        "request_discriminators": [],
                        "stream_abi": null
                    }
                }
            },
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

        let result = reconcile(&openapi, &bindings).expect("reconcile");
        assert_eq!(
            result["create_members"].binding.as_deref(),
            Some("raw_create_members")
        );
        assert_eq!(
            result["update_metrics"].binding.as_deref(),
            Some("raw_update_metrics")
        );
        assert_eq!(result["pause"].binding.as_deref(), Some("raw_pause"));
    }

    #[test]
    fn optional_nullable_root_requires_exact_two_option_layers_and_no_constraints() {
        let operation = serde_json::json!({
            "operationId": "pause",
            "requestBody": {
                "content": {"application/json": {"schema": {
                    "anyOf": [
                        {"$ref": "#/components/schemas/PauseRequest"},
                        {"type": "null"}
                    ]
                }}}
            },
            "responses": {"204": {"description": "done"}}
        });
        let shape = request_shape(&operation).expect("exact nullable root");
        assert_eq!(
            shape.body,
            Some(RequestBodyShape::OptionalNullableModel(
                RequestMediaDefinition::Json,
                "PauseRequest".into()
            ))
        );

        let constrained = serde_json::json!({
            "operationId": "pause",
            "requestBody": {
                "content": {"application/json": {"schema": {
                    "anyOf": [
                        {"$ref": "#/components/schemas/PauseRequest"},
                        {"type": "null"}
                    ],
                    "minProperties": 1
                }}}
            },
            "responses": {"204": {"description": "done"}}
        });
        assert_eq!(
            request_shape(&constrained).expect_err("constraints remain fail closed"),
            "request.inline_or_unresolved"
        );

        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {"/pause": {"post": operation}},
            "components": {"schemas": {
                "PauseRequest": {
                    "type": "object",
                    "properties": {"reason": {"type": "string"}}
                }
            }}
        }));
        let mut shallow = v3_operation(
            "raw_pause",
            "pause",
            "POST",
            "/pause",
            OperationBindingKind::CallShape,
        );
        shallow.parameters = vec![ParameterBinding {
            name: "request".into(),
            type_name: "Option<PauseRequest>".into(),
        }];
        assert_eq!(
            reconcile(
                &openapi,
                &v3_bindings(BTreeMap::from([("raw_pause".into(), shallow)]))
            )
            .expect("reconcile")["pause"]
                .binding,
            None
        );

        let mut exact = v3_operation(
            "raw_pause",
            "pause",
            "POST",
            "/pause",
            OperationBindingKind::CallShape,
        );
        exact.parameters = vec![ParameterBinding {
            name: "request".into(),
            type_name: "Option<Option<PauseRequest>>".into(),
        }];
        assert_eq!(
            reconcile(
                &openapi,
                &v3_bindings(BTreeMap::from([("raw_pause".into(), exact)]))
            )
            .expect("reconcile")["pause"]
                .binding
                .as_deref(),
            Some("raw_pause")
        );
    }

    #[test]
    fn rejects_binding_when_generated_request_discriminator_mutation_is_unproved() {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/ping": {
                    "get": {
                        "operationId": "ping",
                        "responses": {"204": {"description": "done"}}
                    }
                }
            }
        }));
        let mut raw = v3_operation(
            "raw_ping",
            "ping",
            "GET",
            "/ping",
            OperationBindingKind::CallShape,
        );
        raw.metadata
            .as_mut()
            .expect("v3 metadata")
            .request_discriminator_unproven = true;
        let result = reconcile(
            &openapi,
            &v3_bindings(BTreeMap::from([("raw_ping".into(), raw)])),
        )
        .expect("reconcile");
        assert_eq!(result["ping"].binding, None);
        assert_eq!(result["ping"].reason, Some("bindings.no_structural_match"));
    }

    #[test]
    fn reconciles_unrelated_binding_name_and_argument_order() {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/articles/{article-id}": {
                    "parameters": [{"name": "article-id", "in": "path", "schema": {"type": "string"}}],
                    "post": {
                        "operationId": "publish_article",
                        "parameters": [{"name": "preview", "in": "query", "schema": {"type": "boolean"}}],
                        "requestBody": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/PublishRequest"}}}},
                        "responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Article"}}}}}
                    }
                }
            }
        }));
        let raw = operation(
            vec![
                ParameterBinding {
                    name: "request".into(),
                    type_name: "PublishRequest".into(),
                },
                ParameterBinding {
                    name: "preview".into(),
                    type_name: "Option<bool>".into(),
                },
                ParameterBinding {
                    name: "article_id".into(),
                    type_name: "impl AsRef<str>".into(),
                },
            ],
            "Article",
        );
        let result = reconcile(
            &openapi,
            &bindings(BTreeMap::from([("call_17".into(), raw)])),
        )
        .expect("reconcile");
        assert_eq!(
            result["publish_article"].binding.as_deref(),
            Some("call_17")
        );
        assert_eq!(result["publish_article"].reason, None);
    }

    #[test]
    fn refuses_ambiguous_bindings_without_name_heuristics() {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/health": {"get": {"operationId": "health", "responses": {"204": {"description": "ok"}}}}
            }
        }));
        let raw = operation(Vec::new(), "()");
        let result = reconcile(
            &openapi,
            &bindings(BTreeMap::from([
                ("health".into(), raw.clone()),
                ("health_raw".into(), raw),
            ])),
        )
        .expect("reconcile");
        assert_eq!(result["health"].binding, None);
        assert_eq!(
            result["health"].reason,
            Some("bindings.source_operation_identity_required")
        );
    }

    #[test]
    fn canonical_source_identity_disambiguates_structurally_identical_operations() {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/health-a": {"get": {
                    "operationId": "health_a",
                    "responses": {"204": {"description": "ok"}}
                }},
                "/health-b": {"get": {
                    "operationId": "health_b",
                    "responses": {"204": {"description": "ok"}}
                }}
            }
        }));
        let bindings = v3_bindings(BTreeMap::from([
            (
                "opaque_a".into(),
                v3_operation(
                    "opaque_a",
                    "health_a",
                    "GET",
                    "/health-a",
                    OperationBindingKind::CallShape,
                ),
            ),
            (
                "opaque_b".into(),
                v3_operation(
                    "opaque_b",
                    "health_b",
                    "GET",
                    "/health-b",
                    OperationBindingKind::CallShape,
                ),
            ),
        ]));

        let result = reconcile(&openapi, &bindings).expect("reconcile");
        assert_eq!(result["health_a"].binding.as_deref(), Some("opaque_a"));
        assert_eq!(result["health_b"].binding.as_deref(), Some("opaque_b"));
        assert_eq!(result["health_a"].reason, None);
        assert_eq!(result["health_b"].reason, None);
    }

    #[test]
    fn canonical_source_identity_rejects_method_or_path_drift() {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/health": {"get": {
                    "operationId": "health",
                    "responses": {"204": {"description": "ok"}}
                }}
            }
        }));
        let bindings = v3_bindings(BTreeMap::from([(
            "opaque_health".into(),
            v3_operation(
                "opaque_health",
                "health",
                "POST",
                "/other",
                OperationBindingKind::CallShape,
            ),
        )]));

        let result = reconcile(&openapi, &bindings).expect("reconcile");
        assert_eq!(result["health"].binding, None);
        assert_eq!(
            result["health"].reason,
            Some("bindings.source_operation_identity_mismatch")
        );
    }

    #[test]
    fn multipart_filename_helper_is_not_a_primary_operation_candidate() {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/upload": {"post": {
                    "operationId": "upload",
                    "responses": {"204": {"description": "ok"}}
                }}
            }
        }));
        let bindings = v3_bindings(BTreeMap::from([
            (
                "opaque_upload".into(),
                v3_operation(
                    "opaque_upload",
                    "upload",
                    "POST",
                    "/upload",
                    OperationBindingKind::CallShape,
                ),
            ),
            (
                "opaque_upload_with_filenames".into(),
                v3_operation(
                    "opaque_upload_with_filenames",
                    "upload",
                    "POST",
                    "/upload",
                    OperationBindingKind::MultipartFilenames,
                ),
            ),
        ]));

        let result = reconcile(&openapi, &bindings).expect("reconcile");
        assert_eq!(result["upload"].binding.as_deref(), Some("opaque_upload"));
        assert_eq!(result["upload"].reason, None);
    }

    #[test]
    fn canonical_representation_reconciles_multiple_success_statuses() {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/item": {"get": {
                    "operationId": "read_item",
                    "responses": {
                        "200": {"content": {"application/json": {
                            "schema": {"$ref": "#/components/schemas/Item"}
                        }}},
                        "206": {"content": {"application/json": {
                            "schema": {"$ref": "#/components/schemas/Item"}
                        }}}
                    }
                }}
            }
        }));
        let bindings = v3_bindings(BTreeMap::from([(
            "opaque_read".into(),
            v3_operation_with_response(
                "opaque_read",
                "read_item",
                "GET",
                "/item",
                OperationBindingKind::CallShape,
                (
                    "Item",
                    ResponseRepresentationBinding::Json {
                        schema_name: "Item".into(),
                        media_type: "application/json".into(),
                    },
                    vec!["200", "206"],
                    None,
                ),
            ),
        )]));

        let result = reconcile(&openapi, &bindings).expect("reconcile");
        assert_eq!(result["read_item"].binding.as_deref(), Some("opaque_read"));
        assert_eq!(result["read_item"].reason, None);
    }

    #[test]
    fn canonical_representation_selects_one_media_from_multi_media_success() {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/export": {"get": {
                    "operationId": "export",
                    "responses": {"200": {"content": {
                        "application/json": {
                            "schema": {"$ref": "#/components/schemas/Export"}
                        },
                        "application/octet-stream": {
                            "schema": {"type": "string", "format": "binary"}
                        }
                    }}}
                }}
            }
        }));
        let bindings = v3_bindings(BTreeMap::from([(
            "opaque_json_export".into(),
            v3_operation_with_response(
                "opaque_json_export",
                "export",
                "GET",
                "/export",
                OperationBindingKind::CallShape,
                (
                    "Export",
                    ResponseRepresentationBinding::Json {
                        schema_name: "Export".into(),
                        media_type: "application/json".into(),
                    },
                    vec!["200"],
                    None,
                ),
            ),
        )]));

        let result = reconcile(&openapi, &bindings).expect("reconcile");
        assert_eq!(
            result["export"].binding.as_deref(),
            Some("opaque_json_export")
        );
        assert_eq!(result["export"].reason, None);
    }

    #[test]
    fn canonical_representation_reconciles_text_success() {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/text": {"get": {
                    "operationId": "read_text",
                    "responses": {"200": {"content": {
                        "text/plain": {"schema": {"type": "string"}}
                    }}}
                }}
            }
        }));
        let bindings = v3_bindings(BTreeMap::from([(
            "opaque_text".into(),
            v3_operation_with_response(
                "opaque_text",
                "read_text",
                "GET",
                "/text",
                OperationBindingKind::CallShape,
                (
                    "String",
                    ResponseRepresentationBinding::Text {
                        media_type: "text/plain".into(),
                    },
                    vec!["200"],
                    None,
                ),
            ),
        )]));

        let result = reconcile(&openapi, &bindings).expect("reconcile");
        assert_eq!(result["read_text"].binding.as_deref(), Some("opaque_text"));
        assert_eq!(result["read_text"].reason, None);
    }

    #[test]
    fn canonical_representation_reconciles_event_stream_success() {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/events": {"get": {
                    "operationId": "events",
                    "responses": {"200": {"content": {
                        "text/event-stream": {
                            "schema": {"$ref": "#/components/schemas/EventEnvelope"}
                        }
                    }}}
                }}
            }
        }));
        let bindings = v3_bindings(BTreeMap::from([(
            "opaque_events".into(),
            v3_operation_with_response(
                "opaque_events",
                "events",
                "GET",
                "/events",
                OperationBindingKind::CallShape,
                (
                    "HttpResponseByteStream",
                    ResponseRepresentationBinding::EventStream {
                        media_type: "text/event-stream".into(),
                    },
                    vec!["200"],
                    Some(StreamBinding {
                        item_type: "bytes::Bytes".into(),
                        error_type: "reqwest::Error".into(),
                        lifetime: "'static".into(),
                    }),
                ),
            ),
        )]));

        let result = reconcile(&openapi, &bindings).expect("reconcile");
        assert_eq!(result["events"].binding.as_deref(), Some("opaque_events"));
        assert_eq!(result["events"].reason, None);
    }

    #[test]
    fn mixed_success_media_requires_transport_identity() {
        let openapi = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/export": {"get": {
                    "operationId": "export",
                    "responses": {"200": {"content": {
                        "application/json": {"schema": {"$ref": "#/components/schemas/Export"}},
                        "application/octet-stream": {"schema": {"type": "string", "format": "binary"}}
                    }}}
                }}
            }
        }));
        let result = reconcile(&openapi, &bindings(BTreeMap::new())).expect("reconcile");
        assert_eq!(
            result["export"].reason,
            Some("transport.source_operation_identity_required")
        );
    }
}
