use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use serde_json::Value;

use crate::contracts::{
    AccessorDefinition, AccessorKindDefinition, Bindings, MapDefinition, ModelDefinition,
    OperationDefinition, ResourceDefinition, ScalarEnumDefinition, SdkDefinition,
    SimpleUnionDefinition, SimpleUnionVariant,
};
use crate::openapi::{OpenApiIndex, ref_name};
use crate::rust_type::{Type, parse_type};
use crate::structural::{
    ScalarFieldShape, ScalarKind as StructuralScalarKind, inline_array_object_item,
    inline_object_union_mapping, raw_scalar_struct_shape, request_object_matches,
    rust_type_matches_schema, scalar_object_shape,
};
use crate::symbols::field_identifier;

const REQUEST_MODEL_UNPROVEN: &str = "capability.request_model_not_structurally_provable";
const RESPONSE_VIEW_UNPROVEN: &str = "capability.response_model_derivation_required";
const RESPONSE_UNION_REQUIRED: &str = "capability.response_union_derivation_required";
const BINARY_RESPONSE_REQUIRED: &str = "capability.binary_response_derivation_required";

type ProjectedModels = Vec<(String, ModelDefinition)>;

#[derive(Debug, Clone)]
enum ProjectedResponse {
    Empty,
    Json {
        name: String,
        models: ProjectedModels,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectedOperation {
    resource_path: Vec<String>,
    public_name: String,
    models: ProjectedModels,
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

fn semantic_pascal_identifier(value: &str) -> Result<String, &'static str> {
    let name: String = value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect();
    if name.is_empty()
        || !name
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
    {
        return Err(RESPONSE_UNION_REQUIRED);
    }
    Ok(name)
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

fn request_non_null_schema(schema: &Value) -> (&Value, bool) {
    let Some(branches) = schema.get("anyOf").and_then(Value::as_array) else {
        return (schema, false);
    };
    if branches.len() != 2 {
        return (schema, false);
    }
    let non_null: Vec<_> = branches
        .iter()
        .filter(|branch| branch.get("type").and_then(Value::as_str) != Some("null"))
        .collect();
    let nulls = branches
        .iter()
        .filter(|branch| branch.get("type").and_then(Value::as_str) == Some("null"))
        .count();
    if non_null.len() == 1 && nulls == 1 {
        (non_null[0], true)
    } else {
        (schema, false)
    }
}

fn request_raw_core(type_name: &str) -> Result<(Type, usize), &'static str> {
    let mut syntax = parse_type(type_name).map_err(|_| REQUEST_MODEL_UNPROVEN)?;
    let mut depth = 0;
    while let Some(inner) = syntax.unary("Option") {
        depth += 1;
        syntax = inner.clone();
    }
    Ok((syntax, depth))
}

fn request_object_models_value(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    schema: &Value,
    source_root: &str,
    source_path: &[String],
    raw: &str,
    public_name: String,
    seen: &mut BTreeSet<(String, String)>,
) -> Result<ProjectedModels, &'static str> {
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or(REQUEST_MODEL_UNPROVEN)?;
    let fields = bindings.structs.get(raw).ok_or(REQUEST_MODEL_UNPROVEN)?;
    let by_name: BTreeMap<_, _> = fields
        .iter()
        .map(|field| (field.name.strip_prefix("r#").unwrap_or(&field.name), field))
        .collect();
    let required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let required_set: BTreeSet<_> = required.iter().map(String::as_str).collect();

    let mut models = Vec::new();
    let mut adapters = IndexMap::new();
    for (field_name, property) in properties {
        let (wire, nullable) = request_non_null_schema(property);
        if required_set.contains(field_name.as_str()) && nullable {
            return Err(REQUEST_MODEL_UNPROVEN);
        }

        let field = by_name
            .get(field_name.as_str())
            .ok_or(REQUEST_MODEL_UNPROVEN)?;
        let (core, _) = request_raw_core(&field.type_name)?;
        let segment = semantic_pascal_identifier(field_name).map_err(|_| REQUEST_MODEL_UNPROVEN)?;
        let child_name = format!("{public_name}{segment}");

        if let Some(reference) = ref_name(wire) {
            models.extend(request_object_models(
                openapi,
                bindings,
                reference,
                &core.spelling,
                child_name.clone(),
                seen,
            )?);
            adapters.insert(field_name.clone(), child_name);
            continue;
        }

        let inline_object = matches!(wire.get("type").and_then(Value::as_str), Some("object"))
            || wire.get("properties").is_some();
        if inline_object {
            let mut child_path = source_path.to_vec();
            child_path.push(field_name.clone());
            models.extend(request_object_models_value(
                openapi,
                bindings,
                wire,
                source_root,
                &child_path,
                &core.spelling,
                child_name.clone(),
                seen,
            )?);
            adapters.insert(field_name.clone(), child_name);
        }
    }

    if !public_model_name_available(&public_name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    models.push((
        public_name,
        ModelDefinition {
            schema: Some(source_root.into()),
            schema_path: (!source_path.is_empty()).then(|| source_path.to_vec()),
            raw: Some(raw.into()),
            constructor: Some(required),
            exclude: None,
            adapters: (!adapters.is_empty()).then_some(adapters),
            union: None,
            simple_union: None,
            type_alias: None,
            map: None,
            scalar_enum: None,
            union_factory: None,
            borrowed: None,
            accessors: None,
        },
    ));
    Ok(models)
}

fn request_object_models(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    schema_name: &str,
    raw: &str,
    public_name: String,
    seen: &mut BTreeSet<(String, String)>,
) -> Result<ProjectedModels, &'static str> {
    if !request_object_matches(openapi, schema_name, raw, bindings) {
        return Err(REQUEST_MODEL_UNPROVEN);
    }

    let pair = (schema_name.to_owned(), raw.to_owned());
    if !seen.insert(pair.clone()) {
        return Err(REQUEST_MODEL_UNPROVEN);
    }

    let result = openapi
        .object_schema(schema_name)
        .map_err(|_| REQUEST_MODEL_UNPROVEN)
        .and_then(|schema| {
            request_object_models_value(
                openapi,
                bindings,
                &schema,
                schema_name,
                &[],
                raw,
                public_name,
                seen,
            )
        });

    seen.remove(&pair);
    result
}

fn request_model(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    operation_id: &str,
    binding: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<Option<(String, ProjectedModels)>, &'static str> {
    let Some(schema_name) = openapi
        .request_schema(operation_id)
        .map_err(|_| REQUEST_MODEL_UNPROVEN)?
    else {
        return Ok(None);
    };
    let raw_binding = bindings
        .operations
        .get(binding)
        .ok_or("bindings.no_structural_match")?;
    let matching: Vec<_> = raw_binding
        .parameters
        .iter()
        .filter(|parameter| {
            request_object_matches(openapi, &schema_name, &parameter.type_name, bindings)
        })
        .collect();
    if matching.len() != 1 {
        return Err(REQUEST_MODEL_UNPROVEN);
    }

    let raw = &matching[0].type_name;
    let name = request_model_name(resource_path, public_name);
    let models = request_object_models(
        openapi,
        bindings,
        &schema_name,
        raw,
        name.clone(),
        &mut BTreeSet::new(),
    )?;
    Ok(Some((name, models)))
}

fn safe_accessor_name(name: &str) -> bool {
    let mut chars = name.chars();
    let valid = matches!(chars.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    valid && field_identifier(name).is_ok_and(|public| public == name)
}

fn scalar_view_accessors(
    wire: BTreeMap<String, ScalarFieldShape>,
) -> Result<IndexMap<String, AccessorDefinition>, &'static str> {
    let mut accessors = IndexMap::new();
    for (field_name, shape) in wire {
        if !safe_accessor_name(&field_name) {
            return Err(RESPONSE_VIEW_UNPROVEN);
        }
        let kind = match (shape.kind, shape.option_depth) {
            (StructuralScalarKind::String, 0) => AccessorKindDefinition::Ref,
            (StructuralScalarKind::String, _) => AccessorKindDefinition::OptionalRef,
            (_, 0) => AccessorKindDefinition::Copy,
            (_, _) => AccessorKindDefinition::OptionalCopy,
        };
        accessors.insert(
            field_name.clone(),
            AccessorDefinition {
                kind,
                path: vec![field_name],
                wrapper: None,
            },
        );
    }
    Ok(accessors)
}

fn response_view_named(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    raw: &str,
    name: String,
) -> Result<(String, ModelDefinition), &'static str> {
    let schema = openapi
        .object_schema(raw)
        .map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
    let wire = scalar_object_shape(&schema).ok_or(RESPONSE_VIEW_UNPROVEN)?;
    let raw_shape = raw_scalar_struct_shape(bindings, raw).ok_or(RESPONSE_VIEW_UNPROVEN)?;
    if wire != raw_shape {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }
    let accessors = scalar_view_accessors(wire)?;

    if !public_model_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    Ok((
        name,
        ModelDefinition {
            schema: None,
            schema_path: None,
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

fn response_view(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    raw: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<(String, ModelDefinition), &'static str> {
    response_view_named(
        openapi,
        bindings,
        raw,
        response_model_name(resource_path, public_name),
    )
}

fn inline_response_view_named(
    bindings: &Bindings,
    schema: &Value,
    raw: &str,
    name: String,
) -> Result<(String, ModelDefinition), &'static str> {
    let wire = scalar_object_shape(schema).ok_or(RESPONSE_VIEW_UNPROVEN)?;
    let raw_shape = raw_scalar_struct_shape(bindings, raw).ok_or(RESPONSE_VIEW_UNPROVEN)?;
    if wire != raw_shape {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }

    let accessors = scalar_view_accessors(wire)?;

    if !public_model_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    Ok((
        name,
        ModelDefinition {
            schema: None,
            schema_path: None,
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

fn inline_response_view(
    bindings: &Bindings,
    schema: &Value,
    raw: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<(String, ModelDefinition), &'static str> {
    inline_response_view_named(
        bindings,
        schema,
        raw,
        response_model_name(resource_path, public_name),
    )
}

fn inline_array_response_model(
    bindings: &Bindings,
    schema: &Value,
    raw: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<(String, ProjectedModels), &'static str> {
    if !bindings.aliases.contains_key(raw) || !rust_type_matches_schema(schema, raw, bindings) {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }

    let name = response_model_name(resource_path, public_name);
    if !public_model_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
    }

    if let Some(raw_item) = inline_array_object_item(schema, raw, bindings) {
        let items = schema.get("items").ok_or(RESPONSE_VIEW_UNPROVEN)?;
        let item_name = format!("{name}Item");
        let (_, mut item_model) =
            inline_response_view_named(bindings, items, &raw_item, item_name.clone())?;
        item_model.borrowed = Some(true);

        let mut accessors = IndexMap::new();
        accessors.insert(
            "iter".into(),
            AccessorDefinition {
                kind: AccessorKindDefinition::Iter,
                path: Vec::new(),
                wrapper: Some(item_name.clone()),
            },
        );
        let root_model = ModelDefinition {
            schema: None,
            schema_path: None,
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
        };
        return Ok((
            name.clone(),
            vec![(item_name, item_model), (name, root_model)],
        ));
    }

    Ok((
        name.clone(),
        vec![(
            name,
            ModelDefinition {
                schema: None,
                schema_path: None,
                raw: Some(raw.into()),
                constructor: None,
                exclude: None,
                adapters: None,
                union: None,
                simple_union: None,
                type_alias: Some(true),
                map: None,
                scalar_enum: None,
                union_factory: None,
                borrowed: None,
                accessors: None,
            },
        )],
    ))
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

fn type_matches_schema(
    schema: &Value,
    syntax: &Type,
    bindings: &Bindings,
    seen_aliases: &mut BTreeSet<String>,
) -> Result<bool, &'static str> {
    if let Some(alias) = bindings.aliases.get(&syntax.spelling) {
        if !seen_aliases.insert(syntax.spelling.clone()) {
            return Err(RESPONSE_VIEW_UNPROVEN);
        }
        let expanded = parse_type(alias).map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
        let matches = type_matches_schema(schema, &expanded, bindings, seen_aliases)?;
        seen_aliases.remove(&syntax.spelling);
        return Ok(matches);
    }

    Ok(match schema.get("type").and_then(Value::as_str) {
        Some("string") => syntax.spelling == "String",
        Some("boolean") => syntax.spelling == "bool",
        Some("integer") => integer_rust_type(&syntax.spelling),
        Some("number") => matches!(syntax.spelling.as_str(), "f32" | "f64"),
        Some("array") => {
            let Some(items) = schema.get("items") else {
                return Ok(false);
            };
            let Some(inner) = syntax.unary("Vec") else {
                return Ok(false);
            };
            type_matches_schema(items, inner, bindings, seen_aliases)?
        }
        Some("object") => {
            let Some(additional) = schema.get("additionalProperties") else {
                return Ok(false);
            };
            syntax.constructor.as_deref() == Some("std::collections::BTreeMap")
                && syntax.arguments.len() == 2
                && syntax.arguments[0].spelling == "String"
                && type_matches_schema(additional, &syntax.arguments[1], bindings, seen_aliases)?
        }
        _ => false,
    })
}

fn alias_response_model(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    raw: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<(String, ModelDefinition), &'static str> {
    let alias = bindings.aliases.get(raw).ok_or(RESPONSE_VIEW_UNPROVEN)?;
    let syntax = parse_type(alias).map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
    let schema = openapi.schema(raw).map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
    let mut seen_aliases = BTreeSet::from([raw.to_owned()]);
    if !type_matches_schema(schema, &syntax, bindings, &mut seen_aliases)? {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }

    let name = response_model_name(resource_path, public_name);
    if !public_model_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    Ok((
        name,
        ModelDefinition {
            schema: None,
            schema_path: None,
            raw: Some(raw.into()),
            constructor: None,
            exclude: None,
            adapters: None,
            union: None,
            simple_union: None,
            type_alias: Some(true),
            map: None,
            scalar_enum: None,
            union_factory: None,
            borrowed: None,
            accessors: None,
        },
    ))
}

fn map_response_model(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    raw: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<(String, ModelDefinition), &'static str> {
    let schema = openapi.schema(raw).map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
    if schema.get("type").and_then(Value::as_str) != Some("object")
        || schema
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|properties| !properties.is_empty())
    {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }
    let additional = schema
        .get("additionalProperties")
        .filter(|value| **value != Value::Bool(false))
        .ok_or(RESPONSE_VIEW_UNPROVEN)?;
    let fields = bindings.structs.get(raw).ok_or(RESPONSE_VIEW_UNPROVEN)?;
    if fields.len() != 1
        || fields[0].name.strip_prefix("r#").unwrap_or(&fields[0].name) != "additional_properties"
    {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }
    let mapping = parse_type(&fields[0].type_name).map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
    if mapping.constructor.as_deref() != Some("std::collections::BTreeMap")
        || mapping.arguments.len() != 2
        || mapping.arguments[0].spelling != "String"
    {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }
    if !type_matches_schema(
        additional,
        &mapping.arguments[1],
        bindings,
        &mut BTreeSet::new(),
    )? {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }

    let name = response_model_name(resource_path, public_name);
    if !public_model_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    Ok((
        name,
        ModelDefinition {
            schema: None,
            schema_path: None,
            raw: Some(raw.into()),
            constructor: None,
            exclude: None,
            adapters: None,
            union: None,
            simple_union: None,
            type_alias: None,
            map: Some(MapDefinition {
                root: raw.into(),
                path: Vec::new(),
            }),
            scalar_enum: None,
            union_factory: None,
            borrowed: None,
            accessors: None,
        },
    ))
}

fn scalar_enum_response_model(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    raw: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<(String, ModelDefinition), &'static str> {
    let schema = openapi.schema(raw).map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
    let values = schema
        .get("enum")
        .and_then(Value::as_array)
        .ok_or(RESPONSE_VIEW_UNPROVEN)?;
    if schema.get("type").and_then(Value::as_str) != Some("string")
        || values.is_empty()
        || values.iter().any(|value| !value.is_string())
    {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }
    let variants = bindings.enums.get(raw).ok_or(RESPONSE_VIEW_UNPROVEN)?;
    if variants.is_empty()
        || variants
            .iter()
            .any(|variant| variant.payload.is_some() || variant.wire_name.is_none())
    {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }
    let expected: BTreeSet<_> = values.iter().filter_map(Value::as_str).collect();
    let actual: BTreeSet<_> = variants
        .iter()
        .filter_map(|variant| variant.wire_name.as_deref())
        .collect();
    if expected.len() != values.len() || actual.len() != variants.len() || expected != actual {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }

    let name = response_model_name(resource_path, public_name);
    if !public_model_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    Ok((
        name,
        ModelDefinition {
            schema: None,
            schema_path: None,
            raw: Some(raw.into()),
            constructor: None,
            exclude: None,
            adapters: None,
            union: None,
            simple_union: None,
            type_alias: None,
            map: None,
            scalar_enum: Some(ScalarEnumDefinition {
                root: raw.into(),
                path: Vec::new(),
            }),
            union_factory: None,
            borrowed: None,
            accessors: None,
        },
    ))
}

fn response_model(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    raw: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<(String, ModelDefinition), &'static str> {
    if bindings.aliases.contains_key(raw) {
        return alias_response_model(openapi, bindings, raw, resource_path, public_name);
    }
    if bindings.structs.contains_key(raw) {
        let schema = openapi.schema(raw).map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
        if schema.get("type").and_then(Value::as_str) == Some("object")
            && schema.get("additionalProperties").is_some()
            && schema
                .get("properties")
                .and_then(Value::as_object)
                .is_none_or(|properties| properties.is_empty())
        {
            return map_response_model(openapi, bindings, raw, resource_path, public_name);
        }
        return response_view(openapi, bindings, raw, resource_path, public_name);
    }
    if bindings.enums.contains_key(raw) {
        return scalar_enum_response_model(openapi, bindings, raw, resource_path, public_name);
    }
    Err(RESPONSE_VIEW_UNPROVEN)
}

fn inline_union_response_model(
    bindings: &Bindings,
    schema: &Value,
    raw_union: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<(String, ProjectedModels), &'static str> {
    let branches = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
        .ok_or(RESPONSE_UNION_REQUIRED)?;
    let mapping =
        inline_object_union_mapping(schema, raw_union, bindings).ok_or(RESPONSE_UNION_REQUIRED)?;
    if mapping.len() != branches.len() {
        return Err(RESPONSE_UNION_REQUIRED);
    }

    let union_name = response_model_name(resource_path, public_name);
    if !public_model_name_available(&union_name, bindings) {
        return Err("capability.public_model_name_collision");
    }

    let mut models = Vec::new();
    let mut variants = IndexMap::new();
    for (index, ((raw_variant, raw_payload), branch)) in
        mapping.into_iter().zip(branches).enumerate()
    {
        let public_variant = format!("Variant{}", index + 1);
        let branch_name = format!("{union_name}{public_variant}");
        let (adapter, branch_model) =
            inline_response_view_named(bindings, branch, &raw_payload, branch_name)
                .map_err(|_| RESPONSE_UNION_REQUIRED)?;
        models.push((adapter.clone(), branch_model));
        variants.insert(
            raw_variant,
            SimpleUnionVariant::Adapted {
                name: public_variant,
                adapter,
            },
        );
    }

    models.push((
        union_name.clone(),
        ModelDefinition {
            schema: None,
            schema_path: None,
            raw: Some(raw_union.into()),
            constructor: None,
            exclude: None,
            adapters: None,
            union: None,
            simple_union: Some(SimpleUnionDefinition {
                bidirectional: true,
                variants,
            }),
            type_alias: None,
            map: None,
            scalar_enum: None,
            union_factory: None,
            borrowed: None,
            accessors: None,
        },
    ));
    Ok((union_name, models))
}

fn union_response_model(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    schema: &Value,
    raw_union: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<(String, ProjectedModels), &'static str> {
    let branches = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
        .ok_or(RESPONSE_UNION_REQUIRED)?;
    if branches.len() < 2 {
        return Err(RESPONSE_UNION_REQUIRED);
    }
    let references = branches
        .iter()
        .map(|branch| ref_name(branch).map(str::to_owned))
        .collect::<Option<Vec<_>>>();
    let Some(references) = references else {
        return inline_union_response_model(
            bindings,
            schema,
            raw_union,
            resource_path,
            public_name,
        );
    };
    let reference_set: BTreeSet<_> = references.iter().cloned().collect();
    if reference_set.len() != references.len() {
        return Err(RESPONSE_UNION_REQUIRED);
    }

    let raw_variants = bindings
        .enums
        .get(raw_union)
        .ok_or(RESPONSE_UNION_REQUIRED)?;
    if raw_variants.len() != references.len()
        || raw_variants.iter().any(|variant| variant.payload.is_none())
    {
        return Err(RESPONSE_UNION_REQUIRED);
    }
    let payload_to_variant: BTreeMap<_, _> = raw_variants
        .iter()
        .filter_map(|variant| {
            variant
                .payload
                .as_ref()
                .map(|payload| (payload.clone(), variant.name.clone()))
        })
        .collect();
    if payload_to_variant.len() != raw_variants.len()
        || payload_to_variant.keys().cloned().collect::<BTreeSet<_>>() != reference_set
    {
        return Err(RESPONSE_UNION_REQUIRED);
    }

    let union_name = response_model_name(resource_path, public_name);
    if !public_model_name_available(&union_name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    let mut models = Vec::new();
    let mut variants = IndexMap::new();
    let mut public_variants = BTreeSet::new();
    for reference in references {
        let public_variant = semantic_pascal_identifier(&reference)?;
        if !public_variants.insert(public_variant.clone()) {
            return Err("capability.public_model_name_collision");
        }
        let branch_name = format!("{union_name}{public_variant}");
        let (adapter, branch_model) =
            response_view_named(openapi, bindings, &reference, branch_name)
                .map_err(|_| RESPONSE_UNION_REQUIRED)?;
        models.push((adapter.clone(), branch_model));
        let raw_variant = payload_to_variant
            .get(&reference)
            .ok_or(RESPONSE_UNION_REQUIRED)?;
        variants.insert(
            raw_variant.clone(),
            SimpleUnionVariant::Adapted {
                name: public_variant,
                adapter,
            },
        );
    }
    models.push((
        union_name.clone(),
        ModelDefinition {
            schema: None,
            schema_path: None,
            raw: Some(raw_union.into()),
            constructor: None,
            exclude: None,
            adapters: None,
            union: None,
            simple_union: Some(SimpleUnionDefinition {
                bidirectional: true,
                variants,
            }),
            type_alias: None,
            map: None,
            scalar_enum: None,
            union_factory: None,
            borrowed: None,
            accessors: None,
        },
    ));
    Ok((union_name, models))
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
        if let Some(raw) = ref_name(schema) {
            if raw_binding.success_type != raw {
                return Err(RESPONSE_VIEW_UNPROVEN);
            }
            let (name, model) = response_model(openapi, bindings, raw, resource_path, public_name)?;
            return Ok(ProjectedResponse::Json {
                name: name.clone(),
                models: vec![(name, model)],
            });
        }
        if schema.get("oneOf").is_some() || schema.get("anyOf").is_some() {
            let (name, models) = union_response_model(
                openapi,
                bindings,
                schema,
                &raw_binding.success_type,
                resource_path,
                public_name,
            )?;
            return Ok(ProjectedResponse::Json { name, models });
        }
        if schema.get("type").and_then(Value::as_str) == Some("object") {
            let (name, model) = inline_response_view(
                bindings,
                schema,
                &raw_binding.success_type,
                resource_path,
                public_name,
            )?;
            return Ok(ProjectedResponse::Json {
                name: name.clone(),
                models: vec![(name, model)],
            });
        }
        if schema.get("type").and_then(Value::as_str) == Some("array") {
            let (name, models) = inline_array_response_model(
                bindings,
                schema,
                &raw_binding.success_type,
                resource_path,
                public_name,
            )?;
            return Ok(ProjectedResponse::Json { name, models });
        }
        return Err(RESPONSE_VIEW_UNPROVEN);
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
    let request_model = request_model(
        openapi,
        bindings,
        operation_id,
        binding,
        &path,
        &public_name,
    )?;
    let request = request_model.as_ref().map(|(name, _)| name.clone());
    let response = response_projection(openapi, bindings, operation, binding, &path, &public_name)?;
    let (response_name, empty_response, response_models) = match response {
        ProjectedResponse::Empty => (None, Some(true), Vec::new()),
        ProjectedResponse::Json { name, models } => (Some(name), None, models),
    };
    let mut models = Vec::new();
    if let Some((_, request_models)) = request_model {
        models.extend(request_models);
    }
    models.extend(response_models);

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
