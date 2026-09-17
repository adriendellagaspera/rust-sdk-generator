use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use serde_json::Value;

use crate::contracts::{
    AccessorDefinition, AccessorKindDefinition, Bindings, ModelDefinition, OperationDefinition,
    ResourceDefinition, SdkDefinition,
};
use crate::openapi::{OpenApiIndex, ref_name};
use crate::rust_type::parse_type;
use crate::symbols::field_identifier;

const REQUEST_MODEL_UNPROVEN: &str = "capability.request_model_not_structurally_provable";
const RESPONSE_VIEW_UNPROVEN: &str = "capability.response_model_derivation_required";
const RESPONSE_UNION_REQUIRED: &str = "capability.response_union_derivation_required";
const BINARY_RESPONSE_REQUIRED: &str = "capability.binary_response_derivation_required";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalarKind {
    String,
    Boolean,
    Integer,
    Number,
}

#[derive(Debug, Clone)]
enum ProjectedResponse {
    Empty,
    Json(String, ModelDefinition),
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectedOperation {
    resource_path: Vec<String>,
    public_name: String,
    models: Vec<(String, ModelDefinition)>,
    operation: OperationDefinition,
}

fn pascal_identifier(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

fn resource_name(path: &[String]) -> String {
    path.iter()
        .map(|segment| pascal_identifier(segment))
        .collect()
}

fn request_model_name(resource_path: &[String], public_name: &str) -> String {
    format!(
        "{}{}Request",
        pascal_identifier(public_name),
        resource_name(resource_path)
    )
}

fn response_model_name(resource_path: &[String], public_name: &str) -> String {
    format!(
        "{}{}Response",
        pascal_identifier(public_name),
        resource_name(resource_path)
    )
}

fn public_model_name_available(name: &str, bindings: &Bindings) -> bool {
    !name.is_empty()
        && !bindings.structs.contains_key(name)
        && !bindings.enums.contains_key(name)
        && !bindings.aliases.contains_key(name)
}

fn request_model(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    operation_id: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<Option<(String, ModelDefinition)>, &'static str> {
    let Some(raw) = openapi
        .request_schema(operation_id)
        .map_err(|_| REQUEST_MODEL_UNPROVEN)?
    else {
        return Ok(None);
    };
    let schema = openapi
        .object_schema(&raw)
        .map_err(|_| REQUEST_MODEL_UNPROVEN)?;
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or(REQUEST_MODEL_UNPROVEN)?;
    let fields = bindings.structs.get(&raw).ok_or(REQUEST_MODEL_UNPROVEN)?;

    let wire_fields: BTreeSet<_> = properties.keys().map(String::as_str).collect();
    let raw_fields: BTreeSet<_> = fields
        .iter()
        .map(|field| field.name.strip_prefix("r#").unwrap_or(&field.name))
        .collect();
    if wire_fields != raw_fields {
        return Err(REQUEST_MODEL_UNPROVEN);
    }

    let required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let required_set: BTreeSet<_> = required.iter().map(String::as_str).collect();
    for field in fields {
        let name = field.name.strip_prefix("r#").unwrap_or(&field.name);
        if !required_set.contains(name)
            && parse_type(&field.type_name)
                .map_err(|_| REQUEST_MODEL_UNPROVEN)?
                .unary("Option")
                .is_none()
        {
            return Err(REQUEST_MODEL_UNPROVEN);
        }
    }

    let name = request_model_name(resource_path, public_name);
    if !public_model_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    Ok(Some((
        name,
        ModelDefinition {
            raw: Some(raw),
            constructor: Some(required),
            exclude: None,
            adapters: None,
            union: None,
            simple_union: None,
            type_alias: None,
            map: None,
            scalar_enum: None,
            union_factory: None,
            borrowed: None,
            accessors: None,
        },
    )))
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

fn direct_scalar_schema(schema: &Value) -> Option<ScalarKind> {
    match schema.get("type").and_then(Value::as_str)? {
        "string" => Some(ScalarKind::String),
        "boolean" => Some(ScalarKind::Boolean),
        "integer" => Some(ScalarKind::Integer),
        "number" => Some(ScalarKind::Number),
        _ => None,
    }
}

fn rust_scalar(type_name: &str) -> Result<(ScalarKind, bool), &'static str> {
    let syntax = parse_type(type_name).map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
    let (inner, optional) = if let Some(inner) = syntax.unary("Option") {
        if inner.unary("Option").is_some() {
            return Err(RESPONSE_VIEW_UNPROVEN);
        }
        (inner.spelling.as_str(), true)
    } else {
        (syntax.spelling.as_str(), false)
    };
    let kind = match inner {
        "String" => ScalarKind::String,
        "bool" => ScalarKind::Boolean,
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" => ScalarKind::Integer,
        "f32" | "f64" => ScalarKind::Number,
        _ => return Err(RESPONSE_VIEW_UNPROVEN),
    };
    Ok((kind, optional))
}

fn safe_accessor_name(name: &str) -> bool {
    let mut chars = name.chars();
    let valid = matches!(chars.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    valid && field_identifier(name).is_ok_and(|public| public == name)
}

fn response_view(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    raw: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<(String, ModelDefinition), &'static str> {
    let schema = openapi
        .object_schema(raw)
        .map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or(RESPONSE_VIEW_UNPROVEN)?;
    let fields = bindings.structs.get(raw).ok_or(RESPONSE_VIEW_UNPROVEN)?;
    let by_name: BTreeMap<_, _> = fields
        .iter()
        .map(|field| (field.name.strip_prefix("r#").unwrap_or(&field.name), field))
        .collect();
    let wire_fields: BTreeSet<_> = properties.keys().map(String::as_str).collect();
    let raw_fields: BTreeSet<_> = by_name.keys().copied().collect();
    if wire_fields != raw_fields || by_name.len() != fields.len() {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }

    let required: BTreeSet<_> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !required.iter().all(|name| wire_fields.contains(name)) {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }

    let mut accessors = IndexMap::new();
    let mut names: Vec<_> = properties.keys().collect();
    names.sort();
    for name in names {
        if !safe_accessor_name(name) {
            return Err(RESPONSE_VIEW_UNPROVEN);
        }
        let (wire_kind, nullable) =
            scalar_schema(&properties[name]).ok_or(RESPONSE_VIEW_UNPROVEN)?;
        let field = by_name[name.as_str()];
        let (raw_kind, optional) = rust_scalar(&field.type_name)?;
        let expected_optional = !required.contains(name.as_str()) || nullable;
        if wire_kind != raw_kind || expected_optional != optional {
            return Err(RESPONSE_VIEW_UNPROVEN);
        }
        let kind = match (raw_kind, optional) {
            (ScalarKind::String, false) => AccessorKindDefinition::Ref,
            (ScalarKind::String, true) => AccessorKindDefinition::OptionalRef,
            (_, false) => AccessorKindDefinition::Copy,
            (_, true) => AccessorKindDefinition::OptionalCopy,
        };
        accessors.insert(
            name.clone(),
            AccessorDefinition {
                kind,
                path: vec![name.clone()],
                wrapper: None,
            },
        );
    }

    let name = response_model_name(resource_path, public_name);
    if !public_model_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    Ok((
        name,
        ModelDefinition {
            raw: Some(raw.into()),
            constructor: None,
            exclude: None,
            adapters: None,
            union: None,
            simple_union: None,
            type_alias: None,
            map: None,
            scalar_enum: None,
            union_factory: None,
            borrowed: Some(false),
            accessors: Some(accessors),
        },
    ))
}

fn response_projection(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    operation: &Value,
    binding: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<ProjectedResponse, &'static str> {
    let raw_binding = bindings
        .operations
        .get(binding)
        .ok_or("bindings.no_structural_match")?;
    let success: Vec<_> = operation
        .get("responses")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|responses| responses.iter())
        .filter(|(status, _)| status.starts_with('2'))
        .collect();
    if success.len() != 1 {
        return Err("response.multiple_success_contracts");
    }
    let content = success[0].1.get("content").and_then(Value::as_object);
    let Some(content) = content.filter(|content| !content.is_empty()) else {
        return if raw_binding.success_type == "()" {
            Ok(ProjectedResponse::Empty)
        } else {
            Err("capability.empty_response_not_structurally_provable")
        };
    };
    if content.len() != 1 {
        return Err("transport.source_operation_identity_required");
    }
    let (media, payload) = content.iter().next().expect("one response representation");
    let schema = payload
        .get("schema")
        .ok_or("response.inline_or_unresolved")?;
    if media == "application/json" {
        let Some(raw) = ref_name(schema) else {
            return Err(RESPONSE_UNION_REQUIRED);
        };
        if raw_binding.success_type != raw {
            return Err(RESPONSE_VIEW_UNPROVEN);
        }
        let (name, model) = response_view(openapi, bindings, raw, resource_path, public_name)?;
        return Ok(ProjectedResponse::Json(name, model));
    }
    let binary = schema.get("type").and_then(Value::as_str) == Some("string")
        && schema.get("format").and_then(Value::as_str) == Some("binary");
    if binary {
        Err(BINARY_RESPONSE_REQUIRED)
    } else {
        Err("capability.response_projection_not_implemented")
    }
}

pub(crate) fn project_operation(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    operation_id: &str,
    binding: &str,
    public_path: &str,
) -> Result<ProjectedOperation, &'static str> {
    let mut path: Vec<_> = public_path.split('.').map(str::to_owned).collect();
    let public_name = path.pop().ok_or("surface.invalid_public_path")?;
    if path.is_empty() || public_name.is_empty() {
        return Err("surface.invalid_public_path");
    }

    let operation = openapi
        .operation(operation_id)
        .map_err(|_| "openapi.unknown_operation")?;
    let request_model = request_model(openapi, bindings, operation_id, &path, &public_name)?;
    let request = request_model.as_ref().map(|(name, _)| name.clone());
    let response = response_projection(openapi, bindings, operation, binding, &path, &public_name)?;
    let (response_name, empty_response, response_model) = match response {
        ProjectedResponse::Empty => (None, Some(true), None),
        ProjectedResponse::Json(name, model) => (Some(name.clone()), None, Some((name, model))),
    };
    let mut models = Vec::new();
    if let Some(model) = request_model {
        models.push(model);
    }
    if let Some(model) = response_model {
        models.push(model);
    }

    Ok(ProjectedOperation {
        resource_path: path,
        public_name,
        models,
        operation: OperationDefinition {
            operation_id: operation_id.into(),
            raw_method: Some(binding.into()),
            request,
            response: response_name,
            empty_response,
            binary_response: None,
            stream: None,
            request_overrides: None,
        },
    })
}

pub(crate) fn insert_projection(
    definition: &mut SdkDefinition,
    projected: ProjectedOperation,
) -> Result<(), &'static str> {
    let model_names: BTreeSet<_> = projected.models.iter().map(|(name, _)| name).collect();
    if model_names.len() != projected.models.len()
        || model_names
            .iter()
            .any(|name| definition.models.contains_key(*name))
    {
        return Err("capability.public_model_name_collision");
    }

    for depth in 1..=projected.resource_path.len() {
        let path = projected.resource_path[..depth].to_vec();
        let module = path.join("_");
        let name = resource_name(&path);
        if name.is_empty() {
            return Err("surface.invalid_public_path");
        }
        if let Some(existing) = definition.resources.get(&module)
            && (existing.path.as_ref() != Some(&path) || existing.name != name)
        {
            return Err("surface.resource_name_collision");
        }
    }
    let leaf_module = projected.resource_path.join("_");
    if definition
        .resources
        .get(&leaf_module)
        .is_some_and(|resource| resource.operations.contains_key(&projected.public_name))
    {
        return Err("surface.public_path_collision");
    }

    for (name, model) in projected.models {
        definition.models.insert(name, model);
    }
    for depth in 1..=projected.resource_path.len() {
        let path = projected.resource_path[..depth].to_vec();
        let module = path.join("_");
        let name = resource_name(&path);
        definition
            .resources
            .entry(module)
            .or_insert_with(|| ResourceDefinition {
                name,
                path: Some(path),
                operations: IndexMap::new(),
            });
    }
    definition
        .resources
        .get_mut(&leaf_module)
        .expect("leaf resource inserted")
        .operations
        .insert(projected.public_name, projected.operation);
    Ok(())
}
