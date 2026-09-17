use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::contracts::{Bindings, OpenApi, OperationBinding};
use crate::error::{GenerationError, Result};
use crate::openapi::{OpenApiIndex, ref_name};
use crate::rust_type::parse_type;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationMatch {
    pub binding: Option<String>,
    pub reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestShape {
    parameters: BTreeSet<String>,
    body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResponseShape {
    Empty,
    JsonRef(String),
    JsonUnion(BTreeSet<String>),
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
    for (path, path_item) in paths {
        let Some(path_item) = path_item.as_object() else {
            continue;
        };
        let path_parameters = path_item
            .get("parameters")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for method in ["get", "put", "post", "delete", "patch", "head", "options"] {
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
            normalized.insert("x-sdk-path".into(), Value::String(path.clone()));
            normalized.insert("x-sdk-method".into(), Value::String(method.into()));
            operations.insert(operation_id.into(), Value::Object(normalized));
        }
    }
    Ok(operations)
}

fn normalized_parameter_name(value: &str) -> String {
    value.replace('-', "_")
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
        if !matches!(parameter.get("in").and_then(Value::as_str), Some("path" | "query")) {
            return Err("request.parameter_binding_unsupported");
        }
        let Some(name) = parameter.get("name").and_then(Value::as_str) else {
            return Err("request.parameter_binding_unsupported");
        };
        if !parameters.insert(normalized_parameter_name(name)) {
            return Err("request.parameter_binding_unsupported");
        }
    }

    let body = match operation
        .get("requestBody")
        .and_then(|body| body.get("content"))
        .and_then(Value::as_object)
    {
        None => None,
        Some(content) if content.is_empty() => None,
        Some(content) if content.len() == 1 && content.contains_key("application/json") => content
            .get("application/json")
            .and_then(|payload| payload.get("schema"))
            .and_then(ref_name)
            .map(str::to_owned)
            .ok_or("request.inline_or_unresolved")?
            .into(),
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
    let content = success[0]
        .1
        .get("content")
        .and_then(Value::as_object);
    let Some(content) = content.filter(|content| !content.is_empty()) else {
        return Ok(ResponseShape::Empty);
    };
    if content.len() != 1 {
        return Err("transport.source_operation_identity_required");
    }
    let (media, payload) = content.iter().next().expect("one response media");
    let schema = payload.get("schema").unwrap_or(&Value::Null);
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
        return Err("response.inline_or_unresolved");
    }
    if schema.get("type").and_then(Value::as_str) == Some("string")
        && schema.get("format").and_then(Value::as_str) == Some("binary")
    {
        return Ok(ResponseShape::Binary);
    }
    Err("response.non_json_success")
}

fn raw_parameter_name(value: &str) -> &str {
    value.strip_prefix("r#").unwrap_or(value)
}

fn response_matches(shape: &ResponseShape, binding: &OperationBinding, bindings: &Bindings) -> bool {
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

fn binding_matches(
    request: &RequestShape,
    response: &ResponseShape,
    binding: &OperationBinding,
    bindings: &Bindings,
) -> bool {
    let mut body_index = None;
    if let Some(body) = &request.body {
        let matching: Vec<_> = binding
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, parameter)| parameter.type_name == *body)
            .map(|(index, _)| index)
            .collect();
        if matching.len() != 1 {
            return false;
        }
        body_index = matching.first().copied();
    }

    let raw_names: Vec<_> = binding
        .parameters
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != body_index)
        .map(|(_, parameter)| raw_parameter_name(&parameter.name).to_owned())
        .collect();
    let raw_set: BTreeSet<_> = raw_names.iter().cloned().collect();
    raw_names.len() == raw_set.len()
        && raw_set == request.parameters
        && response_matches(response, binding, bindings)
}

pub(crate) fn reconcile(
    openapi: &OpenApi,
    bindings: &Bindings,
) -> Result<BTreeMap<String, OperationMatch>> {
    let operations = normalized_operations(openapi)?;
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
        let response = match response_shape(operation) {
            Ok(shape) => shape,
            Err(reason) => {
                reasons.insert(operation_id.clone(), reason);
                continue;
            }
        };
        candidates.insert(
            operation_id.clone(),
            bindings
                .operations
                .iter()
                .filter(|(_, binding)| binding_matches(&request, &response, binding, bindings))
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
                    reason: Some(*reason),
                },
            );
            continue;
        }
        let matches = &candidates[operation_id];
        let outcome = if matches.is_empty() {
            OperationMatch {
                binding: None,
                reason: Some("bindings.no_structural_match"),
            }
        } else if matches.len() != 1
            || reverse
                .get(&matches[0])
                .is_some_and(|operations| operations.len() != 1)
        {
            OperationMatch {
                binding: None,
                reason: Some("bindings.source_operation_identity_required"),
            }
        } else {
            OperationMatch {
                binding: Some(matches[0].clone()),
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
    use crate::contracts::{BindingLayout, ClientBinding, ParameterBinding};

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
        }
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
                ParameterBinding { name: "request".into(), type_name: "PublishRequest".into() },
                ParameterBinding { name: "preview".into(), type_name: "Option<bool>".into() },
                ParameterBinding { name: "article_id".into(), type_name: "impl AsRef<str>".into() },
            ],
            "Article",
        );
        let result = reconcile(
            &openapi,
            &bindings(BTreeMap::from([("call_17".into(), raw)])),
        )
        .expect("reconcile");
        assert_eq!(result["publish_article"].binding.as_deref(), Some("call_17"));
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
