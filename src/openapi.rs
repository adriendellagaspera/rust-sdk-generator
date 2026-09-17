use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::OpenApi;
use crate::error::{GenerationError, Result};

const SCHEMA_ANNOTATIONS: &[&str] = &[
    "deprecated",
    "description",
    "example",
    "examples",
    "readOnly",
    "title",
    "writeOnly",
];

fn error(code: &'static str, message: impl Into<String>) -> GenerationError {
    GenerationError::new(code, message)
}

fn component_ref(value: Option<&Value>) -> Option<&str> {
    let prefix = "#/components/schemas/";
    value?.as_str()?.strip_prefix(prefix).filter(|name| !name.is_empty())
}

pub(crate) fn ref_name(schema: &Value) -> Option<&str> {
    component_ref(schema.get("$ref"))
}

fn literal_values(schema: &Map<String, Value>) -> Option<Vec<Value>> {
    if let Some(value) = schema.get("const") {
        return Some(vec![value.clone()]);
    }
    schema.get("enum")?.as_array().cloned()
}

fn json_value_key(value: &Value) -> String {
    serde_json::to_string(value).expect("JSON value is serializable")
}

fn nullable_property_inner(schema: &Map<String, Value>) -> Option<Map<String, Value>> {
    let branches = schema.get("anyOf")?.as_array()?;
    let allowed: BTreeSet<&str> = SCHEMA_ANNOTATIONS
        .iter()
        .copied()
        .chain(["anyOf", "default"])
        .collect();
    if branches.len() != 2 || schema.keys().any(|key| !allowed.contains(key.as_str())) {
        return None;
    }
    let null = serde_json::json!({"type": "null"});
    let null_count = branches.iter().filter(|branch| **branch == null).count();
    let values: Vec<_> = branches.iter().filter(|branch| **branch != null).collect();
    if null_count != 1 || values.len() != 1 {
        return None;
    }
    let mut inner = values[0].as_object()?.clone();
    for key in SCHEMA_ANNOTATIONS {
        if let Some(value) = schema.get(*key)
            && !inner.contains_key(*key)
        {
            inner.insert((*key).into(), value.clone());
        }
    }
    if schema.get("default").is_some_and(|value| !value.is_null())
        && !inner.contains_key("default")
    {
        inner.insert("default".into(), schema["default"].clone());
    }
    Some(inner)
}

fn intersect_property_schema(
    left: &Map<String, Value>,
    right: &Map<String, Value>,
    context: &str,
) -> Result<Map<String, Value>> {
    if left == right {
        return Ok(left.clone());
    }
    let left_inner = nullable_property_inner(left);
    let right_inner = nullable_property_inner(right);
    if let (Some(left_inner), None) = (&left_inner, &right_inner) {
        return intersect_property_schema(left_inner, right, context);
    }
    if let (None, Some(right_inner)) = (&left_inner, &right_inner) {
        return intersect_property_schema(left, right_inner, context);
    }

    let ignored: BTreeSet<&str> = SCHEMA_ANNOTATIONS
        .iter()
        .copied()
        .chain(["const", "default", "enum"])
        .collect();
    let left_constraints: BTreeMap<_, _> = left
        .iter()
        .filter(|(key, _)| !ignored.contains(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let right_constraints: BTreeMap<_, _> = right
        .iter()
        .filter(|(key, _)| !ignored.contains(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if left_constraints != right_constraints {
        return Err(error(
            "openapi.property_conflict",
            format!("conflicting OpenAPI property {context} across allOf"),
        ));
    }

    let left_values = literal_values(left);
    let right_values = literal_values(right);
    let values = match (left_values, right_values) {
        (None, None) => None,
        (None, Some(values)) | (Some(values), None) => Some(values),
        (Some(left_values), Some(right_values)) => {
            let right_keys: BTreeSet<_> = right_values.iter().map(json_value_key).collect();
            let values: Vec<_> = left_values
                .into_iter()
                .filter(|value| right_keys.contains(&json_value_key(value)))
                .collect();
            if values.is_empty() {
                return Err(error(
                    "openapi.property_conflict",
                    format!("conflicting OpenAPI property {context} across allOf"),
                ));
            }
            Some(values)
        }
    };

    let mut result: Map<String, Value> = left_constraints.into_iter().collect();
    for source in [left, right] {
        for key in SCHEMA_ANNOTATIONS {
            if let Some(value) = source.get(*key) {
                result.insert((*key).into(), value.clone());
            }
        }
    }
    if let Some(values) = values {
        let allowed: BTreeSet<_> = values.iter().map(json_value_key).collect();
        result.insert("enum".into(), Value::Array(values));
        for source in [right, left] {
            if let Some(default) = source.get("default")
                && allowed.contains(&json_value_key(default))
            {
                result.insert("default".into(), default.clone());
                break;
            }
        }
    } else if let Some(default) = right.get("default").or_else(|| left.get("default")) {
        result.insert("default".into(), default.clone());
    }
    Ok(result)
}

fn merge_object_shapes(parts: &[Map<String, Value>], context: &str) -> Result<Value> {
    let mut properties = Map::new();
    let mut required = Vec::<String>::new();
    let mut additional_properties = None;

    for part in parts {
        if let Some(part_properties) = part.get("properties") {
            let part_properties = part_properties.as_object().ok_or_else(|| {
                error(
                    "openapi.invalid_object",
                    format!("invalid OpenAPI object shape: {context}"),
                )
            })?;
            for (name, schema) in part_properties {
                if let Some(previous) = properties.get(name) {
                    let previous = previous.as_object().ok_or_else(|| {
                        error(
                            "openapi.invalid_property",
                            format!("invalid OpenAPI property {context}.{name}"),
                        )
                    })?;
                    let schema = schema.as_object().ok_or_else(|| {
                        error(
                            "openapi.invalid_property",
                            format!("invalid OpenAPI property {context}.{name}"),
                        )
                    })?;
                    properties.insert(
                        name.clone(),
                        Value::Object(intersect_property_schema(previous, schema, &format!("{context}.{name}"))?),
                    );
                } else {
                    properties.insert(name.clone(), schema.clone());
                }
            }
        }
        if let Some(names) = part.get("required") {
            for name in names.as_array().ok_or_else(|| {
                error(
                    "openapi.invalid_object",
                    format!("invalid OpenAPI object shape: {context}"),
                )
            })? {
                let name = name.as_str().ok_or_else(|| {
                    error(
                        "openapi.invalid_object",
                        format!("invalid OpenAPI object shape: {context}"),
                    )
                })?;
                if !required.iter().any(|existing| existing == name) {
                    required.push(name.into());
                }
            }
        }
        if let Some(candidate) = part.get("additionalProperties") {
            if let Some(previous) = &additional_properties
                && previous != candidate
            {
                return Err(error(
                    "openapi.additional_properties_conflict",
                    format!("conflicting additionalProperties for OpenAPI object {context}"),
                ));
            }
            additional_properties = Some(candidate.clone());
        }
    }

    let missing: Vec<_> = required
        .iter()
        .filter(|name| !properties.contains_key(name.as_str()))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(error(
            "openapi.undefined_required",
            format!("OpenAPI object {context} requires undefined properties: {missing:?}"),
        ));
    }

    let mut result = Map::new();
    result.insert("type".into(), Value::String("object".into()));
    result.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        result.insert(
            "required".into(),
            Value::Array(required.into_iter().map(Value::String).collect()),
        );
    }
    if let Some(additional_properties) = additional_properties {
        result.insert("additionalProperties".into(), additional_properties);
    }
    Ok(Value::Object(result))
}

#[derive(Debug, Clone)]
pub(crate) struct OpenApiIndex {
    schemas: BTreeMap<String, Value>,
    operations: BTreeMap<String, Value>,
}

impl OpenApiIndex {
    pub fn new(openapi: &OpenApi) -> Result<Self> {
        let root = openapi.0.as_object().ok_or_else(|| {
            error("openapi.invalid", "OpenAPI document must be a JSON object")
        })?;
        let schemas = root
            .get("components")
            .and_then(Value::as_object)
            .and_then(|components| components.get("schemas"))
            .and_then(Value::as_object)
            .map(|schemas| {
                schemas
                    .iter()
                    .map(|(name, schema)| (name.clone(), schema.clone()))
                    .collect()
            })
            .unwrap_or_default();

        let mut operations = BTreeMap::new();
        if let Some(paths) = root.get("paths").and_then(Value::as_object) {
            for (path, path_item) in paths {
                let Some(path_item) = path_item.as_object() else {
                    continue;
                };
                let path_parameters = path_item
                    .get("parameters")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for method in ["get", "put", "post", "delete", "options", "head", "patch", "trace"] {
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
                    if operations
                        .insert(operation_id.into(), Value::Object(normalized))
                        .is_some()
                    {
                        return Err(error(
                            "openapi.duplicate_operation",
                            format!("duplicate OpenAPI operationId: {operation_id}"),
                        ));
                    }
                }
            }
        }
        Ok(Self { schemas, operations })
    }

    pub fn schema(&self, name: &str) -> Result<&Value> {
        self.schemas.get(name).ok_or_else(|| {
            error(
                "openapi.unknown_schema",
                format!("unknown OpenAPI schema: {name}"),
            )
        })
    }

    pub fn operation(&self, operation_id: &str) -> Result<&Value> {
        self.operations.get(operation_id).ok_or_else(|| {
            error(
                "openapi.unknown_operation",
                format!("unknown OpenAPI operation: {operation_id}"),
            )
        })
    }

    pub fn request_schema(&self, operation_id: &str) -> Result<Option<String>> {
        let operation = self.operation(operation_id)?;
        Ok(operation
            .pointer("/requestBody/content/application~1json/schema")
            .and_then(ref_name)
            .map(str::to_owned))
    }

    pub fn object_schema(&self, name: &str) -> Result<Value> {
        fn resolve(
            index: &OpenApiIndex,
            root: &str,
            schema: &Value,
            stack: &mut Vec<String>,
        ) -> Result<Value> {
            let schema = schema.as_object().ok_or_else(|| {
                error(
                    "openapi.invalid_object",
                    format!("OpenAPI schema {root} is not an object"),
                )
            })?;
            let mut parts = Vec::new();
            if let Some(reference_value) = schema.get("$ref") {
                let reference = component_ref(Some(reference_value)).ok_or_else(|| {
                    error(
                        "openapi.unsupported_reference",
                        format!(
                            "unsupported OpenAPI object reference in {root}: {reference_value}"
                        ),
                    )
                })?;
                if stack.iter().any(|item| item == reference) {
                    let mut cycle = stack.clone();
                    cycle.push(reference.into());
                    return Err(error(
                        "openapi.recursive_object",
                        format!("recursive OpenAPI object composition: {}", cycle.join(" -> ")),
                    ));
                }
                stack.push(reference.into());
                parts.push(
                    resolve(index, root, index.schema(reference)?, stack)?
                        .as_object()
                        .expect("resolved object")
                        .clone(),
                );
                stack.pop();
            }
            if let Some(all_of) = schema.get("allOf") {
                let branches = all_of.as_array().ok_or_else(|| {
                    error(
                        "openapi.invalid_all_of",
                        format!("invalid allOf branch in OpenAPI object {root}"),
                    )
                })?;
                for branch in branches {
                    parts.push(
                        resolve(index, root, branch, stack)?
                            .as_object()
                            .expect("resolved object")
                            .clone(),
                    );
                }
            }

            let mut local = Map::new();
            for key in ["type", "properties", "required", "additionalProperties"] {
                if let Some(value) = schema.get(key) {
                    local.insert(key.into(), value.clone());
                }
            }
            if !local.is_empty() {
                let kind = local.get("type").and_then(Value::as_str);
                if !matches!(kind, None | Some("object")) {
                    return Err(error(
                        "openapi.not_object",
                        format!("OpenAPI schema {root} is not an object: {kind:?}"),
                    ));
                }
                if local
                    .get("properties")
                    .is_some_and(|value| !value.is_object())
                    || local.get("required").is_some_and(|value| {
                        value
                            .as_array()
                            .is_none_or(|values| values.iter().any(|field| !field.is_string()))
                    })
                {
                    return Err(error(
                        "openapi.invalid_object",
                        format!("invalid OpenAPI object shape: {root}"),
                    ));
                }
                parts.push(local);
            }
            if parts.is_empty() {
                return Err(error(
                    "openapi.not_object",
                    format!("OpenAPI schema {root} is not an object"),
                ));
            }
            merge_object_shapes(&parts, root)
        }

        let mut stack = vec![name.into()];
        resolve(self, name, self.schema(name)?, &mut stack)
    }

    pub fn union(&self, root: &str, path: &[String]) -> Result<(String, BTreeMap<String, String>)> {
        let mut schema = self.schema(root)?;
        for segment in path {
            schema = if segment == "items" {
                schema.get("items")
            } else {
                schema.get("properties").and_then(|properties| properties.get(segment))
            }
            .ok_or_else(|| {
                error(
                    "openapi.union_path",
                    format!("invalid OpenAPI union path {root}.{}", path.join(".")),
                )
            })?;
            if let Some(branches) = schema.get("anyOf").and_then(Value::as_array) {
                let non_null: Vec<_> = branches
                    .iter()
                    .filter(|branch| branch.get("type") != Some(&Value::String("null".into())))
                    .collect();
                if non_null.len() == 1 && non_null.len() != branches.len() {
                    schema = non_null[0];
                }
            }
        }
        let branches = schema
            .get("oneOf")
            .or_else(|| schema.get("anyOf"))
            .and_then(Value::as_array)
            .ok_or_else(|| error("openapi.union", format!("OpenAPI union missing at {root}")))?;
        let discriminator = schema
            .pointer("/discriminator/propertyName")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                error(
                    "openapi.union_discriminator",
                    format!("OpenAPI union discriminator missing at {root}"),
                )
            })?;
        let explicit = schema
            .pointer("/discriminator/mapping")
            .and_then(Value::as_object);
        if let Some(mapping) = explicit {
            let mut result = BTreeMap::new();
            for (tag, target) in mapping {
                let target = component_ref(Some(target)).ok_or_else(|| {
                    error(
                        "openapi.union_mapping",
                        format!("invalid OpenAPI discriminator mapping for {root}: {target}"),
                    )
                })?;
                result.insert(tag.clone(), target.into());
            }
            return Ok((discriminator.into(), result));
        }

        let mut result = BTreeMap::new();
        for branch in branches {
            let payload = ref_name(branch).ok_or_else(|| {
                error(
                    "openapi.union_branch",
                    format!("OpenAPI union branch is not a component ref at {root}"),
                )
            })?;
            let payload_schema = self.schema(payload)?;
            let tag = payload_schema
                .get("properties")
                .and_then(|properties| properties.get(discriminator))
                .and_then(|property| {
                    property
                        .get("const")
                        .and_then(Value::as_str)
                        .or_else(|| {
                            let values = property.get("enum")?.as_array()?;
                            (values.len() == 1).then(|| values[0].as_str()).flatten()
                        })
                })
                .ok_or_else(|| {
                    error(
                        "openapi.union_tag",
                        format!("cannot infer discriminator tag for {payload}"),
                    )
                })?;
            if result.insert(tag.into(), payload.into()).is_some() {
                return Err(error(
                    "openapi.union_duplicate_tag",
                    format!("duplicate discriminator tag {tag} for {root}"),
                ));
            }
        }
        Ok((discriminator.into(), result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composed() -> OpenApiIndex {
        let value: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/composed-openapi/openapi.json"
        ))
        .expect("fixture JSON");
        OpenApiIndex::new(&OpenApi(value)).expect("valid index")
    }

    #[test]
    fn flattens_local_all_of() {
        let schema = composed()
            .object_schema("NestedCommand")
            .expect("composed object");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], serde_json::json!(["name", "priority"]));
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["properties"]["dry_run"],
            serde_json::json!({"type": "boolean", "enum": [false]})
        );
    }

    #[test]
    fn intersects_nullable_literal_refinement() {
        let schema = composed()
            .object_schema("NonStreamingNullableCommand")
            .expect("compatible refinement");
        assert_eq!(
            schema["properties"]["stream"],
            serde_json::json!({"type": "boolean", "enum": [false], "default": false})
        );
    }

    #[test]
    fn rejects_recursive_composition() {
        let error = composed()
            .object_schema("RecursiveA")
            .expect_err("recursive composition");
        assert_eq!(error.diagnostic.code, "openapi.recursive_object");
    }
}
