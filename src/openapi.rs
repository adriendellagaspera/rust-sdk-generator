use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::error::{GenerationError, Result};
use crate::{OpenApi, RequestMediaDefinition};

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
    value?
        .as_str()?
        .strip_prefix(prefix)
        .filter(|name| !name.is_empty())
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
    if schema.get("default").is_some_and(|value| !value.is_null()) && !inner.contains_key("default")
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

    let values = match (literal_values(left), literal_values(right)) {
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
                        Value::Object(intersect_property_schema(
                            previous,
                            schema,
                            &format!("{context}.{name}"),
                        )?),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StructuredRequestBody {
    pub media: RequestMediaDefinition,
    pub schema: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawRequestBody {
    pub media: RequestMediaDefinition,
    pub type_name: String,
}

#[derive(Debug, Clone)]
pub(crate) struct OpenApiIndex {
    schemas: BTreeMap<String, Value>,
    operations: BTreeMap<String, Value>,
}

impl OpenApiIndex {
    pub fn new(openapi: &OpenApi) -> Result<Self> {
        let root = openapi
            .0
            .as_object()
            .ok_or_else(|| error("openapi.invalid", "OpenAPI document must be a JSON object"))?;
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
                for method in ["get", "put", "post", "delete", "patch", "head", "options"] {
                    let Some(operation) = path_item.get(method).and_then(Value::as_object) else {
                        continue;
                    };
                    let Some(operation_id) = operation.get("operationId").and_then(Value::as_str)
                    else {
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
        Ok(Self {
            schemas,
            operations,
        })
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

    pub fn structured_request_body(
        &self,
        operation_id: &str,
    ) -> Result<Option<StructuredRequestBody>> {
        let operation = self.operation(operation_id)?;
        let Some(content) = operation
            .get("requestBody")
            .and_then(|body| body.get("content"))
            .and_then(Value::as_object)
            .filter(|content| !content.is_empty())
        else {
            return Ok(None);
        };
        if content.len() != 1 {
            return Ok(None);
        }
        let (media_type, payload) = content.iter().next().expect("one request media");
        let media = match media_type.as_str() {
            "application/json" => RequestMediaDefinition::Json,
            "multipart/form-data" => RequestMediaDefinition::MultipartFormData,
            "application/x-www-form-urlencoded" => RequestMediaDefinition::FormUrlencoded,
            _ => return Ok(None),
        };
        let Some(schema) = payload.get("schema").and_then(ref_name) else {
            return Ok(None);
        };
        Ok(Some(StructuredRequestBody {
            media,
            schema: schema.into(),
        }))
    }

    pub fn raw_request_body(&self, operation_id: &str) -> Result<Option<RawRequestBody>> {
        let operation = self.operation(operation_id)?;
        let Some(request_body) = operation.get("requestBody") else {
            return Ok(None);
        };
        let required = request_body
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let Some(content) = request_body
            .get("content")
            .and_then(Value::as_object)
            .filter(|content| content.len() == 1)
        else {
            return Ok(None);
        };
        let (media_type, payload) = content.iter().next().expect("one request media");
        let Some(schema) = payload.get("schema") else {
            return Ok(None);
        };
        let binary = schema.get("type").and_then(Value::as_str) == Some("string")
            && schema.get("format").and_then(Value::as_str) == Some("binary");
        let string = schema.get("type").and_then(Value::as_str) == Some("string")
            && schema.get("format").is_none();

        let (media, inner) = if media_type == "application/octet-stream" && binary {
            (RequestMediaDefinition::OctetStream, "Vec<u8>")
        } else if media_type == "text/plain" && string {
            (RequestMediaDefinition::TextPlain, "String")
        } else if binary {
            (RequestMediaDefinition::Binary, "Vec<u8>")
        } else {
            return Ok(None);
        };
        let type_name = if required {
            inner.to_owned()
        } else {
            format!("Option<{inner}>")
        };
        Ok(Some(RawRequestBody { media, type_name }))
    }

    pub fn request_schema(&self, operation_id: &str) -> Result<Option<String>> {
        Ok(self
            .structured_request_body(operation_id)?
            .filter(|body| body.media == RequestMediaDefinition::Json)
            .map(|body| body.schema))
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
                        format!(
                            "recursive OpenAPI object composition: {}",
                            cycle.join(" -> ")
                        ),
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

    pub fn object_schema_path(&self, root: &str, path: &[String]) -> Result<Value> {
        let mut schema = self.object_schema(root)?;
        let mut context = root.to_owned();

        for segment in path {
            context.push('.');
            context.push_str(segment);
            let property = schema
                .get("properties")
                .and_then(Value::as_object)
                .and_then(|properties| properties.get(segment))
                .cloned()
                .ok_or_else(|| {
                    error(
                        "openapi.object_path",
                        format!("invalid OpenAPI object path {context}"),
                    )
                })?;
            let property = property
                .as_object()
                .and_then(nullable_property_inner)
                .map(Value::Object)
                .unwrap_or(property);

            if let Some(reference) = ref_name(&property) {
                schema = self.object_schema(reference)?;
                continue;
            }

            let object = property.as_object().ok_or_else(|| {
                error(
                    "openapi.not_object",
                    format!("OpenAPI schema at {context} is not an object"),
                )
            })?;
            if object.contains_key("allOf") {
                return Err(error(
                    "openapi.object_path_unsupported",
                    format!("inline allOf object is unsupported at {context}"),
                ));
            }
            let kind = object.get("type").and_then(Value::as_str);
            if !matches!(kind, None | Some("object")) {
                return Err(error(
                    "openapi.not_object",
                    format!("OpenAPI schema at {context} is not an object: {kind:?}"),
                ));
            }
            schema = merge_object_shapes(std::slice::from_ref(object), &context)?;
        }

        Ok(schema)
    }

    pub fn union(&self, root: &str, path: &[String]) -> Result<(String, BTreeMap<String, String>)> {
        let mut schema = self.schema(root)?;
        for segment in path {
            schema = if segment == "items" {
                schema.get("items")
            } else {
                schema
                    .get("properties")
                    .and_then(|properties| properties.get(segment))
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
        let explicit_discriminator = schema
            .pointer("/discriminator/propertyName")
            .and_then(Value::as_str);
        if explicit_discriminator.is_none() {
            let referenced: Vec<String> = branches
                .iter()
                .filter_map(ref_name)
                .map(str::to_owned)
                .collect();
            if !referenced.is_empty() && referenced.len() == branches.len() {
                let mut candidates: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
                for payload in &referenced {
                    if let Some(properties) = self
                        .schema(payload)?
                        .get("properties")
                        .and_then(Value::as_object)
                    {
                        for (property_name, property_schema) in properties {
                            if let Some(value) = property_schema.get("const") {
                                let tag = match value {
                                    Value::String(value) => value.clone(),
                                    Value::Bool(true) => "True".into(),
                                    Value::Bool(false) => "False".into(),
                                    Value::Null => "None".into(),
                                    value => value.to_string(),
                                };
                                candidates
                                    .entry(property_name.clone())
                                    .or_default()
                                    .insert(tag, payload.clone());
                            }
                        }
                    }
                }
                let mut complete = candidates
                    .into_iter()
                    .filter(|(_, mapping)| mapping.len() == referenced.len());
                if let Some((property_name, mapping)) = complete.next()
                    && complete.next().is_none()
                {
                    return Ok((property_name, mapping));
                }
            }
        }
        let discriminator = explicit_discriminator.ok_or_else(|| {
            error(
                "openapi.union_discriminator",
                format!("OpenAPI union discriminator missing at {root}"),
            )
        })?;

        if let Some(mapping) = schema
            .pointer("/discriminator/mapping")
            .and_then(Value::as_object)
        {
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
                    property.get("const").and_then(Value::as_str).or_else(|| {
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

    #[test]
    fn infers_unique_const_discriminator_without_discriminator_object() {
        let value = serde_json::json!({
            "components": {"schemas": {
                "Animal": {"oneOf": [
                    {"$ref": "#/components/schemas/Cat"},
                    {"$ref": "#/components/schemas/Dog"}
                ]},
                "Cat": {"type": "object", "properties": {"kind": {"const": "cat"}}},
                "Dog": {"type": "object", "properties": {"kind": {"const": "dog"}}}
            }}
        });
        let index = OpenApiIndex::new(&OpenApi(value)).expect("valid index");
        let (property, mapping) = index.union("Animal", &[]).expect("inferred union");
        assert_eq!(property, "kind");
        assert_eq!(mapping["cat"], "Cat");
        assert_eq!(mapping["dog"], "Dog");
    }

    #[test]
    fn ignores_trace_operations_to_match_existing_contract() {
        let value = serde_json::json!({
            "paths": {"/trace": {"trace": {"operationId": "trace_only"}}}
        });
        let index = OpenApiIndex::new(&OpenApi(value)).expect("valid index");
        assert!(!index.operations.contains_key("trace_only"));
    }
}
