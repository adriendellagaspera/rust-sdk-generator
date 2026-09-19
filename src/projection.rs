use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use serde_json::Value;

use crate::contracts::{
    AccessorDefinition, AccessorKindDefinition, Bindings, MapDefinition, ModelDefinition,
    OperationDefinition, RequestDiscriminatorValue, RequestMediaDefinition, ResourceDefinition,
    ResponseRepresentationBinding, ResponseRepresentationDefinition, ScalarEnumDefinition,
    SdkDefinition, SimpleUnionDefinition, SimpleUnionVariant, StreamDefinition,
};
use crate::openapi::{OpenApiIndex, ref_name};
use crate::rust_type::{Type, parse_type};
use crate::structural::{
    ScalarFieldShape, ScalarKind as StructuralScalarKind, inline_array_object_item,
    inline_object_union_mapping, multipart_filenames_binding, object_value_matches,
    raw_scalar_struct_shape, request_object_matches, request_optional_boolean_field,
    request_union_mapping, request_union_matches, rust_type_matches_schema,
    scalar_named_object_matches, scalar_object_shape, sse_payload_schema_name,
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
    Text,
    BinaryBuffered,
    BinaryStream,
    Sse {
        stream: StreamDefinition,
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

fn stream_item_model_name(resource_path: &[String], public_name: &str) -> String {
    format!(
        "{}{}StreamItem",
        pascal_identifier(public_name),
        resource_name(resource_path)
    )
}

fn stream_type_name(resource_path: &[String], public_name: &str) -> String {
    format!(
        "{}{}Stream",
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

struct RequestModelContext<'a> {
    openapi: &'a OpenApiIndex,
    bindings: &'a Bindings,
}

fn request_union_models(
    context: &RequestModelContext<'_>,
    schema: &Value,
    source_root: &str,
    source_path: &[String],
    raw_union: &str,
    public_name: String,
    seen: &mut BTreeSet<(String, String)>,
) -> Result<ProjectedModels, &'static str> {
    let mapping = request_union_mapping(context.openapi, schema, raw_union, context.bindings)
        .ok_or(REQUEST_MODEL_UNPROVEN)?;
    if !public_model_name_available(&public_name, context.bindings) {
        return Err("capability.public_model_name_collision");
    }

    let mut models = Vec::new();
    let mut variants = IndexMap::new();
    let mut public_variants = BTreeSet::new();
    for branch in mapping {
        let public_variant =
            semantic_pascal_identifier(&branch.schema).map_err(|_| REQUEST_MODEL_UNPROVEN)?;
        if !public_variants.insert(public_variant.clone()) {
            return Err("capability.public_model_name_collision");
        }
        let adapter = format!("{public_name}{public_variant}");
        models.extend(request_object_models(
            context.openapi,
            context.bindings,
            &branch.schema,
            &branch.raw_payload,
            adapter.clone(),
            seen,
        )?);
        variants.insert(
            branch.raw_variant,
            SimpleUnionVariant::Adapted {
                name: public_variant,
                adapter,
            },
        );
    }

    models.push((
        public_name,
        ModelDefinition {
            schema: Some(source_root.into()),
            schema_path: (!source_path.is_empty()).then(|| source_path.to_vec()),
            raw: Some(raw_union.into()),
            constructor: None,
            exclude: None,
            adapters: None,
            union: None,
            simple_union: Some(SimpleUnionDefinition {
                bidirectional: false,
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
    Ok(models)
}

fn request_union_schema(schema: &Value) -> bool {
    schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
        .is_some_and(|branches| branches.len() >= 2)
}

fn request_object_models_value(
    context: &RequestModelContext<'_>,
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
    let fields = context
        .bindings
        .structs
        .get(raw)
        .ok_or(REQUEST_MODEL_UNPROVEN)?;
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
            let referenced = context
                .openapi
                .schema(reference)
                .map_err(|_| REQUEST_MODEL_UNPROVEN)?;
            if request_union_schema(referenced) {
                if request_union_mapping(
                    context.openapi,
                    referenced,
                    &core.spelling,
                    context.bindings,
                )
                .is_some()
                {
                    models.extend(request_union_models(
                        context,
                        referenced,
                        reference,
                        &[],
                        &core.spelling,
                        child_name.clone(),
                        seen,
                    )?);
                    adapters.insert(field_name.clone(), child_name);
                } else if !request_union_matches(
                    context.openapi,
                    referenced,
                    &core.spelling,
                    context.bindings,
                ) {
                    return Err(REQUEST_MODEL_UNPROVEN);
                }
            } else if referenced.get("properties").is_some() {
                models.extend(request_object_models(
                    context.openapi,
                    context.bindings,
                    reference,
                    &core.spelling,
                    child_name.clone(),
                    seen,
                )?);
                adapters.insert(field_name.clone(), child_name);
            }
            continue;
        }

        if request_union_schema(wire) {
            if request_union_mapping(context.openapi, wire, &core.spelling, context.bindings)
                .is_some()
            {
                let mut child_path = source_path.to_vec();
                child_path.push(field_name.clone());
                models.extend(request_union_models(
                    context,
                    wire,
                    source_root,
                    &child_path,
                    &core.spelling,
                    child_name.clone(),
                    seen,
                )?);
                adapters.insert(field_name.clone(), child_name);
            } else if !request_union_matches(
                context.openapi,
                wire,
                &core.spelling,
                context.bindings,
            ) {
                return Err(REQUEST_MODEL_UNPROVEN);
            }
            continue;
        }

        if wire.get("type").and_then(Value::as_str) == Some("array")
            && let Some(items) = wire.get("items")
            && let Some(raw_union) = core.unary("Vec")
        {
            if let Some(reference) = ref_name(items) {
                let referenced = context
                    .openapi
                    .schema(reference)
                    .map_err(|_| REQUEST_MODEL_UNPROVEN)?;
                if request_union_schema(referenced) {
                    models.extend(request_union_models(
                        context,
                        referenced,
                        reference,
                        &[],
                        &raw_union.spelling,
                        child_name.clone(),
                        seen,
                    )?);
                    adapters.insert(field_name.clone(), child_name);
                    continue;
                }
            } else if request_union_schema(items) {
                let mut child_path = source_path.to_vec();
                child_path.push(field_name.clone());
                child_path.push("items".into());
                models.extend(request_union_models(
                    context,
                    items,
                    source_root,
                    &child_path,
                    &raw_union.spelling,
                    child_name.clone(),
                    seen,
                )?);
                adapters.insert(field_name.clone(), child_name);
                continue;
            }
        }

        if wire.get("properties").is_some() {
            let mut child_path = source_path.to_vec();
            child_path.push(field_name.clone());
            models.extend(request_object_models_value(
                context,
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

    if !public_model_name_available(&public_name, context.bindings) {
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
                &RequestModelContext { openapi, bindings },
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
) -> Result<Option<(String, ProjectedModels, RequestMediaDefinition)>, &'static str> {
    let Some(body) = openapi
        .structured_request_body(operation_id)
        .map_err(|_| REQUEST_MODEL_UNPROVEN)?
    else {
        return Ok(None);
    };
    let schema_name = body.schema;
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
    Ok(Some((name, models, body.media)))
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

fn response_view_for_schema_named(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    schema_name: &str,
    raw: &str,
    name: String,
) -> Result<(String, ModelDefinition), &'static str> {
    let schema = openapi
        .object_schema(schema_name)
        .map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
    let accessors = if let Some(wire) = scalar_object_shape(&schema) {
        match raw_scalar_struct_shape(bindings, raw) {
            Some(raw_shape) if wire == raw_shape => scalar_view_accessors(wire)?,
            _ if request_object_matches(openapi, schema_name, raw, bindings) => IndexMap::new(),
            _ => return Err(RESPONSE_VIEW_UNPROVEN),
        }
    } else if request_object_matches(openapi, schema_name, raw, bindings) {
        IndexMap::new()
    } else {
        return Err(RESPONSE_VIEW_UNPROVEN);
    };

    if !public_model_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    Ok((
        name,
        ModelDefinition {
            schema: Some(schema_name.into()),
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

fn response_view_named(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    raw: &str,
    name: String,
) -> Result<(String, ModelDefinition), &'static str> {
    let (name, mut model) = response_view_for_schema_named(openapi, bindings, raw, raw, name)?;
    model.schema = None;
    Ok((name, model))
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
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    schema: &Value,
    raw: &str,
    name: String,
) -> Result<(String, ModelDefinition), &'static str> {
    let accessors = if let Some(wire) = scalar_object_shape(schema) {
        match raw_scalar_struct_shape(bindings, raw) {
            Some(raw_shape) if wire == raw_shape => scalar_view_accessors(wire)?,
            _ if object_value_matches(openapi, schema, raw, bindings) => IndexMap::new(),
            _ => return Err(RESPONSE_VIEW_UNPROVEN),
        }
    } else if object_value_matches(openapi, schema, raw, bindings) {
        IndexMap::new()
    } else {
        return Err(RESPONSE_VIEW_UNPROVEN);
    };

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
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    schema: &Value,
    raw: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<(String, ModelDefinition), &'static str> {
    inline_response_view_named(
        openapi,
        bindings,
        schema,
        raw,
        response_model_name(resource_path, public_name),
    )
}

fn inline_array_response_model(
    openapi: &OpenApiIndex,
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
            inline_response_view_named(openapi, bindings, items, &raw_item, item_name.clone())?;
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
            inline_response_view_named(openapi, bindings, branch, &raw_payload, branch_name)
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

fn selected_success_response_schemas<'a>(
    operation: &'a Value,
    statuses: &[String],
    media_type: &str,
) -> Result<Vec<&'a Value>, &'static str> {
    let responses = operation
        .get("responses")
        .and_then(Value::as_object)
        .ok_or("response.multiple_success_contracts")?;
    let selected_statuses: Vec<_> = if statuses.is_empty() {
        responses
            .keys()
            .filter(|status| status.starts_with('2'))
            .map(String::as_str)
            .collect()
    } else {
        statuses.iter().map(String::as_str).collect()
    };
    if selected_statuses.is_empty() {
        return Err("response.multiple_success_contracts");
    }
    selected_statuses
        .into_iter()
        .map(|status| {
            responses
                .get(status)
                .and_then(|response| response.get("content"))
                .and_then(Value::as_object)
                .and_then(|content| content.get(media_type))
                .and_then(|payload| payload.get("schema"))
                .ok_or("response.inline_or_unresolved")
        })
        .collect()
}

fn selected_success_responses_are_empty(operation: &Value, statuses: &[String]) -> bool {
    let Some(responses) = operation.get("responses").and_then(Value::as_object) else {
        return false;
    };
    let selected_statuses: Vec<_> = if statuses.is_empty() {
        responses
            .keys()
            .filter(|status| status.starts_with('2'))
            .map(String::as_str)
            .collect()
    } else {
        statuses.iter().map(String::as_str).collect()
    };
    !selected_statuses.is_empty()
        && selected_statuses.into_iter().all(|status| {
            responses.get(status).is_some_and(|response| {
                response
                    .get("content")
                    .and_then(Value::as_object)
                    .is_none_or(|content| content.is_empty())
            })
        })
}

fn text_response_schema(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("string") && schema.get("format").is_none()
}

fn binary_response_schema(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("string")
        && schema.get("format").and_then(Value::as_str) == Some("binary")
}

fn buffered_binary_success_type(type_name: &str) -> bool {
    matches!(type_name, "bytes::Bytes" | "Vec<u8>")
}

fn project_json_response_schema(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    schema: &Value,
    raw_success: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<ProjectedResponse, &'static str> {
    if let Some(raw) = ref_name(schema) {
        if raw_success != raw {
            return Err(RESPONSE_VIEW_UNPROVEN);
        }
        let resolved = openapi.schema(raw).map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
        if resolved.get("oneOf").is_some() || resolved.get("anyOf").is_some() {
            let (name, models) = union_response_model(
                openapi,
                bindings,
                resolved,
                raw_success,
                resource_path,
                public_name,
            )?;
            return Ok(ProjectedResponse::Json { name, models });
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
            raw_success,
            resource_path,
            public_name,
        )?;
        return Ok(ProjectedResponse::Json { name, models });
    }
    if schema.get("type").and_then(Value::as_str) == Some("object") {
        let (name, model) =
            inline_response_view(openapi, bindings, schema, raw_success, resource_path, public_name)?;
        return Ok(ProjectedResponse::Json {
            name: name.clone(),
            models: vec![(name, model)],
        });
    }
    if schema.get("type").and_then(Value::as_str) == Some("array") {
        let (name, models) =
            inline_array_response_model(openapi, bindings, schema, raw_success, resource_path, public_name)?;
        return Ok(ProjectedResponse::Json { name, models });
    }
    Err(RESPONSE_VIEW_UNPROVEN)
}

fn binary_stream_projection(
    operation: &Value,
    raw_binding: &crate::contracts::OperationBinding,
) -> Result<ProjectedResponse, &'static str> {
    let metadata = raw_binding
        .metadata
        .as_ref()
        .ok_or("capability.binary_stream_abi_required")?;
    let ResponseRepresentationBinding::BinaryStream { media_type, .. } = &metadata.representation
    else {
        return Err("capability.binary_stream_abi_required");
    };
    let abi = metadata
        .stream_abi
        .as_ref()
        .ok_or("capability.binary_stream_abi_required")?;
    if raw_binding.success_type != abi.alias
        || abi.item_type != "bytes::Bytes"
        || abi.lifetime != "'static"
        || raw_binding.stream.is_none()
    {
        return Err("capability.binary_stream_abi_required");
    }

    let schemas =
        selected_success_response_schemas(operation, &metadata.success_statuses, media_type)?;
    if schemas.is_empty() || schemas.iter().any(|schema| !binary_response_schema(schema)) {
        return Err("capability.binary_stream_not_structurally_provable");
    }
    Ok(ProjectedResponse::BinaryStream)
}

fn event_stream_projection(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    operation: &Value,
    raw_binding: &crate::contracts::OperationBinding,
    resource_path: &[String],
    public_name: &str,
) -> Result<ProjectedResponse, &'static str> {
    let metadata = raw_binding
        .metadata
        .as_ref()
        .ok_or("capability.event_stream_abi_required")?;
    let ResponseRepresentationBinding::EventStream { media_type } = &metadata.representation else {
        return Err("capability.event_stream_abi_required");
    };
    let abi = metadata
        .stream_abi
        .as_ref()
        .ok_or("capability.event_stream_abi_required")?;
    if raw_binding.success_type != abi.alias
        || abi.item_type != "bytes::Bytes"
        || abi.lifetime != "'static"
    {
        return Err("capability.event_stream_abi_required");
    }

    let schemas =
        selected_success_response_schemas(operation, &metadata.success_statuses, media_type)?;
    if schemas.is_empty() {
        return Err("capability.event_stream_payload_not_structurally_provable");
    }
    let payloads = schemas
        .iter()
        .map(|schema| {
            sse_payload_schema_name(openapi, schema)
                .ok_or("capability.event_stream_payload_not_structurally_provable")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let first = payloads
        .first()
        .ok_or("capability.event_stream_payload_not_structurally_provable")?;
    if payloads.iter().any(|payload| payload != first) {
        return Err("capability.event_stream_payload_not_structurally_provable");
    }

    let raw_candidates = bindings
        .structs
        .keys()
        .filter(|raw| scalar_named_object_matches(openapi, first, raw, bindings))
        .cloned()
        .collect::<Vec<_>>();
    if raw_candidates.len() != 1 {
        return Err("capability.event_stream_payload_not_structurally_provable");
    }
    let raw_item = &raw_candidates[0];
    let wrapper = stream_item_model_name(resource_path, public_name);
    let (_, wrapper_model) =
        response_view_for_schema_named(openapi, bindings, first, raw_item, wrapper.clone())?;

    Ok(ProjectedResponse::Sse {
        stream: StreamDefinition {
            item: raw_item.clone(),
            wrapper: Some(wrapper.clone()),
            type_name: stream_type_name(resource_path, public_name),
        },
        models: vec![(wrapper, wrapper_model)],
    })
}

fn canonical_request_discriminators(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    raw_binding: &crate::contracts::OperationBinding,
    request_model: &mut Option<(String, ProjectedModels, RequestMediaDefinition)>,
) -> Result<Option<IndexMap<String, Option<bool>>>, &'static str> {
    let Some(metadata) = raw_binding.metadata.as_ref() else {
        return Ok(None);
    };
    if metadata.request_discriminators.is_empty() {
        return Ok(None);
    }
    let Some((root_name, models, _)) = request_model.as_mut() else {
        return Err("capability.request_discriminator_projection_required");
    };
    let root = models
        .iter_mut()
        .find(|(name, _)| name == root_name)
        .map(|(_, model)| model)
        .ok_or("capability.request_discriminator_projection_required")?;
    if root
        .schema_path
        .as_ref()
        .is_some_and(|path| !path.is_empty())
    {
        return Err("capability.request_discriminator_projection_required");
    }
    let raw = root
        .raw
        .as_deref()
        .ok_or("capability.request_discriminator_projection_required")?;
    let schema = root.schema.as_deref().unwrap_or(raw);

    let mut overrides = IndexMap::new();
    let mut excluded = root.exclude.clone().unwrap_or_default();
    for discriminator in &metadata.request_discriminators {
        if discriminator.rust_access_path.len() != 1
            || discriminator.field_required
            || discriminator.field_nullable
            || discriminator.field_tri_state
            || discriminator.rust_value_type != "Option<bool>"
        {
            return Err("capability.request_discriminator_projection_required");
        }
        let raw_field = discriminator.rust_access_path[0]
            .strip_prefix("r#")
            .unwrap_or(&discriminator.rust_access_path[0]);
        if raw_field != discriminator.wire_name {
            return Err("capability.request_discriminator_projection_required");
        }
        let RequestDiscriminatorValue::Bool(value) = discriminator.value else {
            return Err("capability.request_discriminator_projection_required");
        };
        if !request_optional_boolean_field(openapi, schema, raw, raw_field, bindings) {
            return Err("capability.request_discriminator_projection_required");
        }
        if overrides
            .insert(raw_field.to_owned(), Some(value))
            .is_some()
        {
            return Err("capability.request_discriminator_projection_required");
        }
        if !excluded.iter().any(|field| field == raw_field) {
            excluded.push(raw_field.to_owned());
        }
    }
    excluded.sort();
    root.exclude = Some(excluded);
    Ok(Some(overrides))
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

    if let Some(metadata) = &raw_binding.metadata {
        return match &metadata.representation {
            ResponseRepresentationBinding::Empty => {
                if raw_binding.success_type == "()"
                    && selected_success_responses_are_empty(operation, &metadata.success_statuses)
                {
                    Ok(ProjectedResponse::Empty)
                } else {
                    Err("capability.empty_response_not_structurally_provable")
                }
            }
            ResponseRepresentationBinding::Json {
                schema_name,
                media_type,
            } => {
                if raw_binding.success_type != *schema_name {
                    return Err(RESPONSE_VIEW_UNPROVEN);
                }
                let schemas = selected_success_response_schemas(
                    operation,
                    &metadata.success_statuses,
                    media_type,
                )?;
                let Some(schema) = schemas.first().copied() else {
                    return Err("response.multiple_success_contracts");
                };
                if schemas.iter().any(|candidate| *candidate != schema) {
                    return Err(RESPONSE_VIEW_UNPROVEN);
                }
                project_json_response_schema(
                    openapi,
                    bindings,
                    schema,
                    &raw_binding.success_type,
                    resource_path,
                    public_name,
                )
            }
            ResponseRepresentationBinding::Text { media_type } => {
                let schemas = selected_success_response_schemas(
                    operation,
                    &metadata.success_statuses,
                    media_type,
                )?;
                if raw_binding.success_type == "String"
                    && schemas.iter().all(|schema| text_response_schema(schema))
                {
                    Ok(ProjectedResponse::Text)
                } else {
                    Err("capability.text_response_not_structurally_provable")
                }
            }
            ResponseRepresentationBinding::BinaryBuffered { media_type, .. } => {
                let schemas = selected_success_response_schemas(
                    operation,
                    &metadata.success_statuses,
                    media_type,
                )?;
                if raw_binding.stream.is_none()
                    && buffered_binary_success_type(&raw_binding.success_type)
                    && schemas.iter().all(|schema| binary_response_schema(schema))
                {
                    Ok(ProjectedResponse::BinaryBuffered)
                } else {
                    Err("capability.buffered_binary_response_not_structurally_provable")
                }
            }
            ResponseRepresentationBinding::EventStream { .. } => event_stream_projection(
                openapi,
                bindings,
                operation,
                raw_binding,
                resource_path,
                public_name,
            ),
            ResponseRepresentationBinding::BinaryStream { .. } => {
                binary_stream_projection(operation, raw_binding)
            }
        };
    }

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
        return project_json_response_schema(
            openapi,
            bindings,
            schema,
            &raw_binding.success_type,
            resource_path,
            public_name,
        );
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
    let mut request_model = request_model(
        openapi,
        bindings,
        operation_id,
        binding,
        &path,
        &public_name,
    )?;
    let raw_binding = bindings
        .operations
        .get(binding)
        .ok_or("bindings.no_structural_match")?;
    let request_overrides =
        canonical_request_discriminators(openapi, bindings, raw_binding, &mut request_model)?;
    let request = request_model.as_ref().map(|(name, _, _)| name.clone());
    let request_media = request_model
        .as_ref()
        .map(|(_, _, media)| *media)
        .or_else(|| {
            openapi
                .raw_request_body(operation_id)
                .ok()
                .flatten()
                .map(|body| body.media)
        });
    let response = response_projection(openapi, bindings, operation, binding, &path, &public_name)?;
    let multipart_filenames = match multipart_filenames_binding(bindings, binding)? {
        Some(_) if request_media == Some(RequestMediaDefinition::MultipartFormData) => Some(true),
        Some(_) => return Err("capability.multipart_filenames_requires_multipart"),
        None => None,
    };
    let canonical_response = bindings.operations[binding]
        .metadata
        .as_ref()
        .map(|metadata| match metadata.representation {
            ResponseRepresentationBinding::Json { .. } => ResponseRepresentationDefinition::Json,
            ResponseRepresentationBinding::Empty => ResponseRepresentationDefinition::Empty,
            ResponseRepresentationBinding::Text { .. } => ResponseRepresentationDefinition::Text,
            ResponseRepresentationBinding::BinaryBuffered { .. } => {
                ResponseRepresentationDefinition::BinaryBuffered
            }
            ResponseRepresentationBinding::EventStream { .. } => {
                ResponseRepresentationDefinition::EventStream
            }
            ResponseRepresentationBinding::BinaryStream { .. } => {
                ResponseRepresentationDefinition::BinaryStream
            }
        });
    let (response_name, empty_response, binary_response, stream, response_models) = match response {
        ProjectedResponse::Empty => (None, Some(true), None, None, Vec::new()),
        ProjectedResponse::Json { name, models } => (Some(name), None, None, None, models),
        ProjectedResponse::Text | ProjectedResponse::BinaryBuffered => {
            (None, None, None, None, Vec::new())
        }
        ProjectedResponse::BinaryStream => (None, None, Some(true), None, Vec::new()),
        ProjectedResponse::Sse { stream, models } => (None, None, None, Some(stream), models),
    };
    let mut models = Vec::new();
    if let Some((_, request_models, _)) = request_model {
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
            request_media,
            response: response_name,
            response_representation: canonical_response,
            empty_response,
            binary_response,
            stream,
            request_overrides,
            multipart_filenames,
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
