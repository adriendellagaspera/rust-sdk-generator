use std::collections::BTreeSet;

use indexmap::IndexMap;
use serde_json::Value;

use crate::contracts::{
    Bindings, ModelDefinition, OperationDefinition, ResourceDefinition, SdkDefinition,
};
use crate::openapi::OpenApiIndex;
use crate::rust_type::parse_type;

const REQUEST_MODEL_UNPROVEN: &str = "capability.request_model_not_structurally_provable";
const RESPONSE_MODEL_REQUIRED: &str = "capability.response_model_derivation_required";
const BINARY_RESPONSE_REQUIRED: &str = "capability.binary_response_derivation_required";

#[derive(Debug, Clone)]
pub(crate) struct ProjectedOperation {
    resource_path: Vec<String>,
    public_name: String,
    request_model: Option<(String, ModelDefinition)>,
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
    if name.is_empty()
        || bindings.structs.contains_key(&name)
        || bindings.enums.contains_key(&name)
        || bindings.aliases.contains_key(&name)
    {
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

fn empty_response(operation: &Value) -> Result<bool, &'static str> {
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
        return Ok(true);
    };
    if content.len() != 1 {
        return Err("transport.source_operation_identity_required");
    }
    let (media, payload) = content.iter().next().expect("one response representation");
    if media == "application/json" {
        return Err(RESPONSE_MODEL_REQUIRED);
    }
    let binary = payload.get("schema").is_some_and(|schema| {
        schema.get("type").and_then(Value::as_str) == Some("string")
            && schema.get("format").and_then(Value::as_str) == Some("binary")
    });
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

    let raw = bindings
        .operations
        .get(binding)
        .ok_or("bindings.no_structural_match")?;
    let operation = openapi
        .operation(operation_id)
        .map_err(|_| "openapi.unknown_operation")?;
    if !empty_response(operation)? || raw.success_type != "()" {
        return Err("capability.empty_response_not_structurally_provable");
    }
    let request_model = request_model(openapi, bindings, operation_id, &path, &public_name)?;
    let request = request_model.as_ref().map(|(name, _)| name.clone());

    Ok(ProjectedOperation {
        resource_path: path,
        public_name,
        request_model,
        operation: OperationDefinition {
            operation_id: operation_id.into(),
            raw_method: Some(binding.into()),
            request,
            response: None,
            empty_response: Some(true),
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
    if let Some((name, _)) = &projected.request_model
        && definition.models.contains_key(name)
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

    if let Some((name, model)) = projected.request_model {
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
