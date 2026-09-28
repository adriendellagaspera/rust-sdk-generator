use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use serde_json::Value;

use crate::contracts::{
    AccessorDefinition, AccessorKindDefinition, Bindings, MapDefinition, ModelDefinition,
    OperationDefinition, RequestDiscriminatorValue, RequestMediaDefinition, ResourceDefinition,
    ResponseRepresentationBinding, ResponseRepresentationDefinition, ScalarEnumDefinition,
    SdkDefinition, SimpleUnionDefinition, SimpleUnionVariant, StreamDefinition,
    StreamVariantDefinition,
};
use crate::openapi::{OpenApiIndex, ref_name};
use crate::reconcile::unconstrained_json_alias_matches;
use crate::rust_type::{Type, parse_type};
use crate::structural::{
    ScalarFieldShape, ScalarKind as StructuralScalarKind, canonical_unconstrained_map_branch,
    constant_enum_response_object_matches, flattened_json_response_object_matches,
    inline_array_object_item, inline_object_union_mapping, legacy_nullable_request_property,
    multipart_filenames_binding, nullable_request_union, object_field_names_match,
    object_value_matches, plain_string_json_alias_matches, raw_scalar_struct_shape,
    redundant_any_of_alternative, referenced_request_object, request_object_matches,
    request_object_matches_with_discriminators, request_optional_boolean_field,
    request_union_mapping, request_union_matches, request_value_union_mapping,
    response_array_union_matches, rust_type_matches_schema, scalar_named_object_matches,
    scalar_object_shape, sse_payload_schema_names,
};
use crate::symbols::field_identifier;

const REQUEST_MODEL_UNPROVEN: &str = "capability.request_model_not_structurally_provable";
const RESPONSE_VIEW_UNPROVEN: &str = "capability.response_model_derivation_required";
const RESPONSE_UNION_REQUIRED: &str = "capability.response_union_derivation_required";
const BINARY_RESPONSE_REQUIRED: &str = "capability.binary_response_derivation_required";

type ProjectedModels = Vec<(String, ModelDefinition)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ModelRepresentation {
    Owned,
    Borrowed,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ModelIdentity {
    source_schema: String,
    representation: ModelRepresentation,
    explicit: bool,
}

#[derive(Debug)]
struct ModelNaming<'a> {
    stable: bool,
    explicit: &'a BTreeMap<String, String>,
    identities: RefCell<BTreeMap<String, BTreeSet<ModelIdentity>>>,
}

impl<'a> ModelNaming<'a> {
    fn new(stable: bool, explicit: &'a BTreeMap<String, String>) -> Self {
        Self {
            stable,
            explicit,
            identities: RefCell::new(BTreeMap::new()),
        }
    }

    fn named(
        &self,
        source_schema: &str,
        representation: ModelRepresentation,
        fallback: String,
    ) -> Result<String, &'static str> {
        if !self.stable {
            return Ok(fallback);
        }
        let explicit = self.explicit.get(source_schema);
        let base = explicit.cloned().unwrap_or(
            semantic_pascal_identifier(source_schema)
                .map_err(|_| "surface.invalid_public_model_identity")?,
        );
        let name = match representation {
            ModelRepresentation::Owned => base,
            ModelRepresentation::Borrowed => format!("{base}Ref"),
        };
        self.identities
            .borrow_mut()
            .entry(name.clone())
            .or_default()
            .insert(ModelIdentity {
                source_schema: source_schema.to_owned(),
                representation,
                explicit: explicit.is_some(),
            });
        Ok(name)
    }

    fn identities(&self) -> BTreeMap<String, BTreeSet<ModelIdentity>> {
        self.identities.borrow().clone()
    }

    fn public_name_available(&self, name: &str, bindings: &Bindings) -> bool {
        public_model_name_available(name, bindings) || self.identities.borrow().contains_key(name)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectionFailure {
    pub code: String,
    pub detail: Option<String>,
}

impl From<&'static str> for ProjectionFailure {
    fn from(code: &'static str) -> Self {
        Self {
            code: code.into(),
            detail: None,
        }
    }
}

type ProjectionResult<T> = std::result::Result<T, ProjectionFailure>;

#[derive(Debug, Clone)]
struct RegisteredProjection {
    model: ModelDefinition,
    identities: BTreeSet<ModelIdentity>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectionRegistry {
    models: BTreeMap<String, RegisteredProjection>,
}

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
    model_identities: BTreeMap<String, BTreeSet<ModelIdentity>>,
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
    naming: &'a ModelNaming<'a>,
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
    let Some(mapping) = request_union_mapping(context.openapi, schema, raw_union, context.bindings)
    else {
        return request_value_union_models(
            context,
            schema,
            source_root,
            source_path,
            raw_union,
            public_name,
            seen,
        );
    };
    if !context
        .naming
        .public_name_available(&public_name, context.bindings)
    {
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
        let adapter = context.naming.named(
            &branch.schema,
            ModelRepresentation::Owned,
            format!("{public_name}{public_variant}"),
        )?;
        models.extend(request_object_models(
            context.openapi,
            context.bindings,
            context.naming,
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

fn request_union_variant_name(schema: &Value) -> Result<String, &'static str> {
    if let Some(reference) = ref_name(schema) {
        return semantic_pascal_identifier(reference).map_err(|_| REQUEST_MODEL_UNPROVEN);
    }
    if let Some(title) = schema.get("title").and_then(Value::as_str)
        && let Ok(name) = semantic_pascal_identifier(title)
    {
        return Ok(name);
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => Ok("String".into()),
        Some("boolean") => Ok("Boolean".into()),
        Some("integer") => Ok("Integer".into()),
        Some("number") => Ok("Number".into()),
        Some("object") => Ok("Object".into()),
        Some("array") => {
            let item = schema.get("items").ok_or(REQUEST_MODEL_UNPROVEN)?;
            let item_name = request_union_variant_name(item).unwrap_or_else(|_| "Value".into());
            Ok(format!("{item_name}List"))
        }
        _ => Err(REQUEST_MODEL_UNPROVEN),
    }
}

fn expand_request_type(
    syntax: Type,
    bindings: &Bindings,
    seen: &mut BTreeSet<String>,
) -> Result<Type, &'static str> {
    if let Some(alias) = bindings.aliases.get(&syntax.spelling) {
        if !seen.insert(syntax.spelling.clone()) {
            return Err(REQUEST_MODEL_UNPROVEN);
        }
        let expanded = parse_type(alias).map_err(|_| REQUEST_MODEL_UNPROVEN)?;
        let result = expand_request_type(expanded, bindings, seen);
        seen.remove(&syntax.spelling);
        return result;
    }
    Ok(syntax)
}

fn request_type_is_public(syntax: Type, bindings: &Bindings, seen: &mut BTreeSet<String>) -> bool {
    let Ok(syntax) = expand_request_type(syntax, bindings, seen) else {
        return false;
    };
    if bindings.symbol_paths.contains_key(&syntax.spelling) {
        return false;
    }
    syntax
        .arguments
        .into_iter()
        .all(|argument| request_type_is_public(argument, bindings, seen))
}

fn request_value_adapter_models(
    context: &RequestModelContext<'_>,
    schema: &Value,
    source_root: &str,
    source_path: &[String],
    raw: &str,
    public_name: String,
    seen: &mut BTreeSet<(String, String)>,
) -> Result<(ProjectedModels, Option<String>), &'static str> {
    let syntax = expand_request_type(
        parse_type(raw).map_err(|_| REQUEST_MODEL_UNPROVEN)?,
        context.bindings,
        &mut BTreeSet::new(),
    )?;

    if let Some(reference) = ref_name(schema) {
        let public_name =
            context
                .naming
                .named(reference, ModelRepresentation::Owned, public_name)?;
        let referenced = context
            .openapi
            .schema(reference)
            .map_err(|_| REQUEST_MODEL_UNPROVEN)?;
        if request_union_schema(referenced) {
            if !request_union_matches(
                context.openapi,
                referenced,
                &syntax.spelling,
                context.bindings,
            ) {
                return Err(REQUEST_MODEL_UNPROVEN);
            }
            return request_union_models(
                context,
                referenced,
                reference,
                &[],
                &syntax.spelling,
                public_name.clone(),
                seen,
            )
            .map(|models| (models, Some(public_name)));
        }
        if referenced_request_object(context.openapi, reference, referenced).is_some()
            && !flattened_json_response_object_matches(
                &context
                    .openapi
                    .object_schema(reference)
                    .map_err(|_| REQUEST_MODEL_UNPROVEN)?,
                &syntax.spelling,
                context.bindings,
            )
        {
            return request_object_models(
                context.openapi,
                context.bindings,
                context.naming,
                reference,
                &syntax.spelling,
                public_name.clone(),
                seen,
            )
            .map(|models| (models, Some(public_name)));
        }
        return request_type_is_public(syntax, context.bindings, &mut BTreeSet::new())
            .then_some((Vec::new(), None))
            .ok_or(REQUEST_MODEL_UNPROVEN);
    }

    if schema.get("type").and_then(Value::as_str) == Some("array") {
        let items = schema.get("items").ok_or(REQUEST_MODEL_UNPROVEN)?;
        let inner = syntax.unary("Vec").ok_or(REQUEST_MODEL_UNPROVEN)?;
        let mut item_path = source_path.to_vec();
        item_path.push("items".into());
        let item_name = format!("{public_name}Item");
        let (models, adapter) = request_value_adapter_models(
            context,
            items,
            source_root,
            &item_path,
            &inner.spelling,
            item_name,
            seen,
        )?;
        if adapter.is_some() {
            return Ok((models, adapter));
        }
        return request_type_is_public(syntax, context.bindings, &mut BTreeSet::new())
            .then_some((Vec::new(), None))
            .ok_or(REQUEST_MODEL_UNPROVEN);
    }

    if request_union_schema(schema) {
        if !request_union_matches(context.openapi, schema, &syntax.spelling, context.bindings) {
            return Err(REQUEST_MODEL_UNPROVEN);
        }
        return request_union_models(
            context,
            schema,
            source_root,
            source_path,
            &syntax.spelling,
            public_name.clone(),
            seen,
        )
        .map(|models| (models, Some(public_name)));
    }

    if canonical_unconstrained_map_branch(schema, &syntax.spelling, context.bindings) {
        if !context
            .naming
            .public_name_available(&public_name, context.bindings)
        {
            return Err("capability.public_model_name_collision");
        }
        return Ok((
            vec![(
                public_name.clone(),
                ModelDefinition {
                    schema: Some(source_root.into()),
                    schema_path: (!source_path.is_empty()).then(|| source_path.to_vec()),
                    raw: Some(syntax.spelling.clone()),
                    constructor: None,
                    exclude: None,
                    adapters: None,
                    union: None,
                    simple_union: None,
                    type_alias: None,
                    map: Some(MapDefinition {
                        root: source_root.into(),
                        path: source_path.to_vec(),
                    }),
                    scalar_enum: None,
                    union_factory: None,
                    borrowed: None,
                    accessors: None,
                },
            )],
            Some(public_name),
        ));
    }

    if schema.get("properties").is_some() {
        return request_object_models_value(
            context,
            schema,
            source_root,
            source_path,
            &syntax.spelling,
            public_name.clone(),
            seen,
        )
        .map(|models| (models, Some(public_name)));
    }

    request_type_is_public(syntax, context.bindings, &mut BTreeSet::new())
        .then_some((Vec::new(), None))
        .ok_or(REQUEST_MODEL_UNPROVEN)
}

fn request_value_union_models(
    context: &RequestModelContext<'_>,
    schema: &Value,
    source_root: &str,
    source_path: &[String],
    raw_union: &str,
    public_name: String,
    seen: &mut BTreeSet<(String, String)>,
) -> Result<ProjectedModels, &'static str> {
    let mapping = request_value_union_mapping(context.openapi, schema, raw_union, context.bindings)
        .ok_or(REQUEST_MODEL_UNPROVEN)?;
    if !context
        .naming
        .public_name_available(&public_name, context.bindings)
    {
        return Err("capability.public_model_name_collision");
    }

    let mut models = Vec::new();
    let mut variants = IndexMap::new();
    let mut public_variants = BTreeSet::new();
    for branch in mapping {
        let public_variant = request_union_variant_name(&branch.schema)?;
        if !public_variants.insert(public_variant.clone()) {
            return Err("capability.public_model_name_collision");
        }
        let branch_adapter = format!("{public_name}{public_variant}");
        let (nested, adapter) = request_value_adapter_models(
            context,
            &branch.schema,
            source_root,
            source_path,
            &branch.raw_payload,
            branch_adapter,
            seen,
        )?;
        models.extend(nested);
        variants.insert(
            branch.raw_variant,
            if let Some(adapter) = adapter {
                SimpleUnionVariant::Adapted {
                    name: public_variant,
                    adapter,
                }
            } else {
                SimpleUnionVariant::Name(public_variant)
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
    redundant_any_of_alternative(schema).is_none()
        && schema
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
    let mut models = Vec::new();
    let mut adapters = IndexMap::new();
    for (field_name, property) in properties {
        let normalized_nullable =
            nullable_request_union(property).or_else(|| legacy_nullable_request_property(property));
        let (non_null, directly_nullable) = request_non_null_schema(property);
        let referenced_nullable = ref_name(property)
            .and_then(|reference| context.openapi.schema(reference).ok())
            .is_some_and(|referenced| {
                nullable_request_union(referenced).is_some()
                    || legacy_nullable_request_property(referenced).is_some()
                    || request_non_null_schema(referenced).1
            });
        let nullable = normalized_nullable.is_some() || directly_nullable || referenced_nullable;
        let wire = normalized_nullable.as_ref().unwrap_or(non_null);
        let field = by_name
            .get(field_name.as_str())
            .ok_or(REQUEST_MODEL_UNPROVEN)?;
        if required.contains(field_name) && nullable && field.serialized_presence.is_none() {
            return Err(REQUEST_MODEL_UNPROVEN);
        }
        let (core, _) = request_raw_core(&field.type_name)?;
        let segment = semantic_pascal_identifier(field_name).map_err(|_| REQUEST_MODEL_UNPROVEN)?;
        let child_fallback = format!("{public_name}{segment}");

        if let Some(reference) = ref_name(wire) {
            let child_name = context.naming.named(
                reference,
                ModelRepresentation::Owned,
                child_fallback.clone(),
            )?;
            let referenced = context
                .openapi
                .schema(reference)
                .map_err(|_| REQUEST_MODEL_UNPROVEN)?;
            if request_union_schema(referenced) {
                if !request_union_matches(
                    context.openapi,
                    referenced,
                    &core.spelling,
                    context.bindings,
                ) {
                    return Err(REQUEST_MODEL_UNPROVEN);
                }
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
            } else if referenced_request_object(context.openapi, reference, referenced).is_some_and(
                |composed| {
                    !flattened_json_response_object_matches(
                        &composed,
                        &core.spelling,
                        context.bindings,
                    )
                },
            ) {
                models.extend(request_object_models(
                    context.openapi,
                    context.bindings,
                    context.naming,
                    reference,
                    &core.spelling,
                    child_name.clone(),
                    seen,
                )?);
                adapters.insert(field_name.clone(), child_name);
            } else if referenced.get("type").and_then(Value::as_str) == Some("string")
                && referenced.get("enum").and_then(Value::as_array).is_some()
                && context.bindings.enums.contains_key(&core.spelling)
                && rust_type_matches_schema(referenced, &core.spelling, context.bindings)
            {
                if !context
                    .naming
                    .public_name_available(&child_name, context.bindings)
                {
                    return Err("capability.public_model_name_collision");
                }
                models.push((
                    child_name.clone(),
                    ModelDefinition {
                        schema: Some(reference.into()),
                        schema_path: None,
                        raw: Some(core.spelling.clone()),
                        constructor: None,
                        exclude: None,
                        adapters: None,
                        union: None,
                        simple_union: None,
                        type_alias: None,
                        map: None,
                        scalar_enum: Some(ScalarEnumDefinition {
                            root: reference.into(),
                            path: Vec::new(),
                        }),
                        union_factory: None,
                        borrowed: None,
                        accessors: None,
                    },
                ));
                adapters.insert(field_name.clone(), child_name);
            }
            continue;
        }

        if request_union_schema(wire) {
            if !request_union_matches(context.openapi, wire, &core.spelling, context.bindings) {
                return Err(REQUEST_MODEL_UNPROVEN);
            }
            let mut child_path = source_path.to_vec();
            child_path.push(field_name.clone());
            let projected = request_union_models(
                context,
                wire,
                source_root,
                &child_path,
                &core.spelling,
                child_fallback.clone(),
                seen,
            )?;
            models.extend(projected);
            adapters.insert(field_name.clone(), child_fallback);
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
                    let child_name = context.naming.named(
                        reference,
                        ModelRepresentation::Owned,
                        child_fallback.clone(),
                    )?;
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
                    child_fallback.clone(),
                    seen,
                )?);
                adapters.insert(field_name.clone(), child_fallback.clone());
                continue;
            }
        }

        if canonical_unconstrained_map_branch(wire, &core.spelling, context.bindings) {
            let mut child_path = source_path.to_vec();
            child_path.push(field_name.clone());
            let (projected, adapter) = request_value_adapter_models(
                context,
                wire,
                source_root,
                &child_path,
                &core.spelling,
                child_fallback.clone(),
                seen,
            )?;
            models.extend(projected);
            if let Some(adapter) = adapter {
                adapters.insert(field_name.clone(), adapter);
            }
            continue;
        }

        if wire.get("properties").is_some()
            && !flattened_json_response_object_matches(wire, &core.spelling, context.bindings)
        {
            let mut child_path = source_path.to_vec();
            child_path.push(field_name.clone());
            models.extend(request_object_models_value(
                context,
                wire,
                source_root,
                &child_path,
                &core.spelling,
                child_fallback.clone(),
                seen,
            )?);
            adapters.insert(field_name.clone(), child_fallback);
        }
    }

    if !context
        .naming
        .public_name_available(&public_name, context.bindings)
    {
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
    naming: &ModelNaming<'_>,
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
                &RequestModelContext {
                    openapi,
                    bindings,
                    naming,
                },
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

fn inline_request_model(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    operation_id: &str,
    binding: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<Option<(String, ProjectedModels, RequestMediaDefinition)>, &'static str> {
    let Some(body) = openapi
        .inline_structured_request_body(operation_id)
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
            object_value_matches(openapi, &body.schema, &parameter.type_name, bindings)
        })
        .collect();
    if matching.len() != 1 {
        return Err(REQUEST_MODEL_UNPROVEN);
    }
    let name = request_model_name(resource_path, public_name);
    if !public_model_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    let model = ModelDefinition {
        schema: None,
        schema_path: None,
        raw: Some(matching[0].type_name.clone()),
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
        accessors: Some(IndexMap::new()),
    };
    Ok(Some((name.clone(), vec![(name, model)], body.media)))
}

fn optional_nullable_json_ref_request_model(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    naming: &ModelNaming<'_>,
    operation_id: &str,
    binding: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<Option<(String, ProjectedModels, RequestMediaDefinition)>, &'static str> {
    let Some(body) = openapi
        .optional_nullable_json_ref_request_body(operation_id)
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
        .filter_map(|parameter| {
            let syntax = parse_type(&parameter.type_name).ok()?;
            let present = syntax.unary("Option")?;
            let nullable = present.unary("Option")?;
            if nullable.unary("Option").is_some()
                || !request_object_matches(openapi, &body.schema, &nullable.spelling, bindings)
            {
                return None;
            }
            Some((parameter, nullable.spelling.clone()))
        })
        .collect();
    if matching.len() != 1 {
        return Err(REQUEST_MODEL_UNPROVEN);
    }
    let raw = &matching[0].1;
    let name = naming.named(
        &body.schema,
        ModelRepresentation::Owned,
        request_model_name(resource_path, public_name),
    )?;
    if !naming.public_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    // The exact inner raw object is structurally proven. Keep an owned opaque
    // public view rather than inventing field construction through an alias.
    let model = ModelDefinition {
        schema: Some(body.schema),
        schema_path: None,
        raw: Some(raw.clone()),
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
        accessors: Some(IndexMap::new()),
    };
    Ok(Some((name.clone(), vec![(name, model)], body.media)))
}

fn required_json_schema_request_model(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    operation_id: &str,
    binding: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<Option<(String, ProjectedModels, RequestMediaDefinition)>, &'static str> {
    let Some(body) = openapi
        .required_json_schema_request_body(operation_id)
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
        .filter(|parameter| rust_type_matches_schema(&body.schema, &parameter.type_name, bindings))
        .collect();
    if matching.len() != 1 {
        return Err(REQUEST_MODEL_UNPROVEN);
    }
    let name = request_model_name(resource_path, public_name);
    if !public_model_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    let model = ModelDefinition {
        schema: None,
        schema_path: None,
        raw: Some(matching[0].type_name.clone()),
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
        accessors: Some(IndexMap::new()),
    };
    Ok(Some((name.clone(), vec![(name, model)], body.media)))
}

fn request_model(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    naming: &ModelNaming<'_>,
    operation_id: &str,
    binding: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<Option<(String, ProjectedModels, RequestMediaDefinition)>, &'static str> {
    if let Some(projected) = inline_request_model(
        openapi,
        bindings,
        operation_id,
        binding,
        resource_path,
        public_name,
    )? {
        return Ok(Some(projected));
    }
    if let Some(projected) = optional_nullable_json_ref_request_model(
        openapi,
        bindings,
        naming,
        operation_id,
        binding,
        resource_path,
        public_name,
    )? {
        return Ok(Some(projected));
    }
    if let Some(projected) = required_json_schema_request_model(
        openapi,
        bindings,
        operation_id,
        binding,
        resource_path,
        public_name,
    )? {
        return Ok(Some(projected));
    }
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
    let discriminators = raw_binding
        .metadata
        .as_ref()
        .map(|metadata| metadata.request_discriminators.as_slice())
        .unwrap_or_default();
    let matching: Vec<_> = raw_binding
        .parameters
        .iter()
        .filter(|parameter| {
            request_object_matches_with_discriminators(
                openapi,
                &schema_name,
                &parameter.type_name,
                bindings,
                discriminators,
            )
        })
        .collect();
    if matching.len() != 1 {
        return Err(REQUEST_MODEL_UNPROVEN);
    }

    let raw = &matching[0].type_name;
    let name = naming.named(
        &schema_name,
        ModelRepresentation::Owned,
        request_model_name(resource_path, public_name),
    )?;
    if openapi
        .object_schema(&schema_name)
        .ok()
        .is_some_and(|schema| flattened_json_response_object_matches(&schema, raw, bindings))
    {
        // The wire shape is fully proven, but a generated constructor would
        // necessarily omit arbitrary additional members. Preserve the exact
        // raw request as an owned opaque view instead.
        if !naming.public_name_available(&name, bindings) {
            return Err("capability.public_model_name_collision");
        }
        let model = ModelDefinition {
            schema: Some(schema_name),
            schema_path: None,
            raw: Some(raw.clone()),
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
            accessors: Some(IndexMap::new()),
        };
        return Ok(Some((name.clone(), vec![(name, model)], body.media)));
    }
    let models = match request_object_models(
        openapi,
        bindings,
        naming,
        &schema_name,
        raw,
        name.clone(),
        &mut BTreeSet::new(),
    ) {
        Ok(models) => models,
        Err(REQUEST_MODEL_UNPROVEN)
            if request_object_matches_with_discriminators(
                openapi,
                &schema_name,
                raw,
                bindings,
                discriminators,
            ) =>
        {
            // Do not synthesize a lossy constructor for an otherwise exact raw
            // request shape. An owned public view still supports From<Raw>
            // and into_raw(), without claiming that its fields are constructible.
            if !naming.public_name_available(&name, bindings) {
                return Err("capability.public_model_name_collision");
            }
            vec![(
                name.clone(),
                ModelDefinition {
                    schema: Some(schema_name.clone()),
                    schema_path: None,
                    raw: Some(raw.clone()),
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
                    accessors: Some(IndexMap::new()),
                },
            )]
        }
        Err(reason) => return Err(reason),
    };
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

fn unwrap_nullable_schema(schema: &Value) -> &Value {
    let Some(branches) = schema.get("anyOf").and_then(Value::as_array) else {
        return schema;
    };
    if branches.len() != 2 {
        return schema;
    }
    let non_null: Vec<_> = branches
        .iter()
        .filter(|branch| branch.get("type").and_then(Value::as_str) != Some("null"))
        .collect();
    if non_null.len() == 1 {
        non_null[0]
    } else {
        schema
    }
}

struct ResponseModelContext<'a> {
    openapi: &'a OpenApiIndex,
    bindings: &'a Bindings,
    naming: &'a ModelNaming<'a>,
}

// A child is projected only after the complete parent wire shape has been
// proven. The active raw-type set prevents recursive schemas from expanding
// indefinitely; no partially projected model escapes on failure.
fn response_object_models(
    context: &ResponseModelContext<'_>,
    schema: &Value,
    raw: &str,
    name: String,
    borrowed: bool,
    active: &mut BTreeSet<String>,
) -> Result<ProjectedModels, &'static str> {
    if !active.insert(raw.to_owned()) {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }
    let result = response_object_models_inner(context, schema, raw, name, borrowed, active);
    active.remove(raw);
    result
}

fn response_object_models_inner(
    context: &ResponseModelContext<'_>,
    schema: &Value,
    raw: &str,
    name: String,
    borrowed: bool,
    active: &mut BTreeSet<String>,
) -> Result<ProjectedModels, &'static str> {
    let openapi = context.openapi;
    let bindings = context.bindings;
    let naming = context.naming;
    if !naming.public_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    let proven = object_value_matches(openapi, schema, raw, bindings)
        || object_field_names_match(schema, raw, bindings)
        || constant_enum_response_object_matches(schema, raw, bindings);
    if !proven {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or(RESPONSE_VIEW_UNPROVEN)?;
    let fields = bindings.structs.get(raw).ok_or(RESPONSE_VIEW_UNPROVEN)?;
    let mut children = Vec::new();
    let mut accessors = IndexMap::new();
    for (field_name, property) in properties {
        if !safe_accessor_name(field_name) {
            return Err(RESPONSE_VIEW_UNPROVEN);
        }
        let field = fields
            .iter()
            .find(|field| {
                field
                    .wire_name
                    .as_deref()
                    .unwrap_or_else(|| field.name.strip_prefix("r#").unwrap_or(&field.name))
                    == field_name
            })
            .ok_or(RESPONSE_VIEW_UNPROVEN)?;
        let syntax = parse_type(&field.type_name).map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
        let (syntax, optional) = if let Some(inner) = syntax.unary("Option") {
            (inner, true)
        } else {
            (&syntax, false)
        };
        // A second Option represents absent versus explicit null. The existing
        // accessors cannot erase that distinction, so decline this shape.
        if syntax.unary("Option").is_some() {
            continue;
        }
        let (value, collection) = if let Some(inner) = syntax.unary("Vec") {
            (inner, true)
        } else {
            (syntax, false)
        };
        let non_null_source = unwrap_nullable_schema(property);
        let non_null = if let Some(reference) = ref_name(non_null_source) {
            openapi
                .schema(reference)
                .map_err(|_| RESPONSE_VIEW_UNPROVEN)?
        } else {
            non_null_source
        };
        let item_source = if collection {
            non_null.get("items").ok_or(RESPONSE_VIEW_UNPROVEN)?
        } else {
            non_null_source
        };
        let source_reference = ref_name(item_source);
        let item_schema = if let Some(reference) = source_reference {
            openapi
                .schema(reference)
                .map_err(|_| RESPONSE_VIEW_UNPROVEN)?
        } else if collection {
            non_null.get("items").ok_or(RESPONSE_VIEW_UNPROVEN)?
        } else {
            non_null
        };
        let field_proven = rust_type_matches_schema(item_schema, &value.spelling, bindings)
            || (item_schema.get("properties").is_some()
                && bindings.structs.contains_key(&value.spelling))
            || ((item_schema.get("oneOf").is_some() || item_schema.get("anyOf").is_some())
                && request_union_matches(openapi, item_schema, &value.spelling, bindings));
        if !field_proven {
            continue;
        }
        let child_fallback = format!("{name}{}", semantic_pascal_identifier(field_name)?);
        let (kind, wrapper) = if bindings.structs.contains_key(&value.spelling)
            && item_schema.get("properties").is_some()
        {
            let child_schema = if let Some(reference) = ref_name(if collection {
                non_null.get("items").ok_or(RESPONSE_VIEW_UNPROVEN)?
            } else {
                unwrap_nullable_schema(property)
            }) {
                openapi
                    .object_schema(reference)
                    .map_err(|_| RESPONSE_VIEW_UNPROVEN)?
            } else {
                item_schema.clone()
            };
            let child_name = if let Some(reference) = source_reference {
                naming.named(
                    reference,
                    ModelRepresentation::Borrowed,
                    child_fallback.clone(),
                )?
            } else {
                child_fallback.clone()
            };
            let Ok(child_models) = response_object_models(
                context,
                &child_schema,
                &value.spelling,
                child_name.clone(),
                true,
                active,
            ) else {
                continue;
            };
            children.extend(child_models);
            (
                match (collection, optional) {
                    (false, false) => AccessorKindDefinition::View,
                    (false, true) => AccessorKindDefinition::OptionalView,
                    (true, false) => AccessorKindDefinition::Iter,
                    (true, true) => AccessorKindDefinition::OptionalIter,
                },
                Some(child_name),
            )
        } else if bindings.enums.contains_key(&value.spelling)
            && (item_schema.get("oneOf").is_some() || item_schema.get("anyOf").is_some())
        {
            let union_name = if let Some(reference) = source_reference {
                naming.named(
                    reference,
                    ModelRepresentation::Owned,
                    child_fallback.clone(),
                )?
            } else {
                child_fallback.clone()
            };
            let Ok((union_name, union_models)) = union_response_model(
                openapi,
                bindings,
                naming,
                item_schema,
                &value.spelling,
                union_name,
            ) else {
                continue;
            };
            children.extend(union_models);
            (
                match (collection, optional) {
                    (false, false) => AccessorKindDefinition::Converted,
                    (false, true) => AccessorKindDefinition::OptionalConverted,
                    (true, false) => AccessorKindDefinition::ConvertedIter,
                    (true, true) => return Err(RESPONSE_VIEW_UNPROVEN),
                },
                Some(union_name),
            )
        } else if bindings.enums.contains_key(&value.spelling) && !collection {
            let enum_name = if let Some(reference) = source_reference {
                naming.named(
                    reference,
                    ModelRepresentation::Owned,
                    child_fallback.clone(),
                )?
            } else {
                child_fallback.clone()
            };
            let enum_model = if item_schema.get("enum").is_some()
                && item_schema.get("type").and_then(Value::as_str) == Some("string")
            {
                let values = item_schema
                    .get("enum")
                    .and_then(Value::as_array)
                    .ok_or(RESPONSE_VIEW_UNPROVEN)?;
                let variants = bindings
                    .enums
                    .get(&value.spelling)
                    .ok_or(RESPONSE_VIEW_UNPROVEN)?;
                if values.len() != variants.len()
                    || variants.iter().any(|variant| {
                        variant.payload.is_some()
                            || !values
                                .iter()
                                .any(|v| v.as_str() == variant.wire_name.as_deref())
                    })
                {
                    return Err(RESPONSE_VIEW_UNPROVEN);
                }
                ModelDefinition {
                    scalar_enum: Some(ScalarEnumDefinition {
                        root: value.spelling.clone(),
                        path: vec![],
                    }),
                    ..empty_response_model(&value.spelling, false)
                }
            } else {
                return Err(RESPONSE_VIEW_UNPROVEN);
            };
            children.push((enum_name.clone(), enum_model));
            (
                if optional {
                    AccessorKindDefinition::OptionalConverted
                } else {
                    AccessorKindDefinition::Converted
                },
                Some(enum_name),
            )
        } else if value.spelling == "String" {
            (
                if optional {
                    AccessorKindDefinition::OptionalRef
                } else {
                    AccessorKindDefinition::Ref
                },
                None,
            )
        } else if matches!(
            value.spelling.as_str(),
            "bool" | "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "f32" | "f64"
        ) && !collection
        {
            (
                if optional {
                    AccessorKindDefinition::OptionalCopy
                } else {
                    AccessorKindDefinition::Copy
                },
                None,
            )
        } else {
            // The parent is exact, but an unsupported child remains opaque.
            continue;
        };
        // Optionality is driven by the raw Option representation and structural
        // proof, including OpenAPI nullable fields.
        accessors.insert(
            field_name.clone(),
            AccessorDefinition {
                kind,
                path: vec![field.name.clone()],
                wrapper,
            },
        );
    }
    if accessors.is_empty() && !properties.is_empty() {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }
    children.push((
        name,
        ModelDefinition {
            accessors: Some(accessors),
            borrowed: Some(borrowed),
            ..empty_response_model(raw, borrowed)
        },
    ));
    Ok(children)
}

fn empty_response_model(raw: &str, borrowed: bool) -> ModelDefinition {
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
        borrowed: Some(borrowed),
        accessors: None,
    }
}

fn response_view_for_schema_named(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    naming: &ModelNaming<'_>,
    schema_name: &str,
    raw: &str,
    name: String,
) -> Result<(String, ProjectedModels), &'static str> {
    let schema = openapi
        .object_schema(schema_name)
        .map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
    if schema
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|properties| {
            properties
                .values()
                .any(|property| property.get("const").is_some())
        })
        && !constant_enum_response_object_matches(&schema, raw, bindings)
    {
        return Err(RESPONSE_VIEW_UNPROVEN);
    }
    let context = ResponseModelContext {
        openapi,
        bindings,
        naming,
    };
    if let Ok(mut models) = response_object_models(
        &context,
        &schema,
        raw,
        name.clone(),
        false,
        &mut BTreeSet::new(),
    ) {
        models.last_mut().ok_or(RESPONSE_VIEW_UNPROVEN)?.1.schema = Some(schema_name.into());
        return Ok((name, models));
    }
    if !naming.public_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    let accessors = if let Some(wire) = scalar_object_shape(&schema) {
        match raw_scalar_struct_shape(bindings, raw) {
            Some(raw_shape) if wire == raw_shape => scalar_view_accessors(wire)?,
            _ if request_object_matches(openapi, schema_name, raw, bindings)
                || constant_enum_response_object_matches(&schema, raw, bindings) =>
            {
                IndexMap::new()
            }
            _ => return Err(RESPONSE_VIEW_UNPROVEN),
        }
    } else if object_field_names_match(&schema, raw, bindings)
        || flattened_json_response_object_matches(&schema, raw, bindings)
    {
        IndexMap::new()
    } else {
        return Err(RESPONSE_VIEW_UNPROVEN);
    };
    Ok((
        name.clone(),
        vec![(
            name,
            ModelDefinition {
                schema: Some(schema_name.into()),
                accessors: Some(accessors),
                borrowed: Some(false),
                ..empty_response_model(raw, false)
            },
        )],
    ))
}

fn response_view_named(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    naming: &ModelNaming<'_>,
    raw: &str,
    name: String,
) -> Result<(String, ProjectedModels), &'static str> {
    response_view_for_schema_named(openapi, bindings, naming, raw, raw, name)
}

fn response_view(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    naming: &ModelNaming<'_>,
    raw: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<(String, ProjectedModels), &'static str> {
    let mut name = naming.named(
        raw,
        ModelRepresentation::Owned,
        response_model_name(resource_path, public_name),
    )?;
    if !naming.stable && name == raw {
        name.push_str("View");
    }
    response_view_named(openapi, bindings, naming, raw, name)
}

fn inline_response_view_named(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    naming: &ModelNaming<'_>,
    schema: &Value,
    raw: &str,
    name: String,
) -> Result<(String, ProjectedModels), &'static str> {
    if object_value_matches(openapi, schema, raw, bindings) {
        let context = ResponseModelContext {
            openapi,
            bindings,
            naming,
        };
        let models = response_object_models(
            &context,
            schema,
            raw,
            name.clone(),
            false,
            &mut BTreeSet::new(),
        )?;
        return Ok((name, models));
    }
    let accessors = if let Some(wire) = scalar_object_shape(schema) {
        match raw_scalar_struct_shape(bindings, raw) {
            Some(raw_shape) if wire == raw_shape => scalar_view_accessors(wire)?,
            _ if object_value_matches(openapi, schema, raw, bindings) => IndexMap::new(),
            _ => return Err(RESPONSE_VIEW_UNPROVEN),
        }
    } else if object_field_names_match(schema, raw, bindings) {
        IndexMap::new()
    } else {
        return Err(RESPONSE_VIEW_UNPROVEN);
    };

    if !naming.public_name_available(&name, bindings) {
        return Err("capability.public_model_name_collision");
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
                type_alias: None,
                map: None,
                scalar_enum: None,
                union_factory: None,
                borrowed: Some(false),
                accessors: Some(accessors),
            },
        )],
    ))
}

fn inline_response_view(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    naming: &ModelNaming<'_>,
    schema: &Value,
    raw: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<(String, ProjectedModels), &'static str> {
    inline_response_view_named(
        openapi,
        bindings,
        naming,
        schema,
        raw,
        response_model_name(resource_path, public_name),
    )
}

fn inline_array_response_model(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    naming: &ModelNaming<'_>,
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

    let items = schema.get("items").ok_or(RESPONSE_VIEW_UNPROVEN)?;
    let inline_item = inline_array_object_item(schema, raw, bindings);
    let named_item = ref_name(items).filter(|reference| bindings.structs.contains_key(*reference));
    if let Some(raw_item) = inline_item.as_deref().or(named_item) {
        let item_fallback = format!("{name}Item");
        let item_name = if inline_item.is_some() {
            item_fallback
        } else {
            naming.named(raw_item, ModelRepresentation::Borrowed, item_fallback)?
        };
        let (_, mut item_models) = if inline_item.is_some() {
            inline_response_view_named(
                openapi,
                bindings,
                naming,
                items,
                raw_item,
                item_name.clone(),
            )?
        } else {
            response_view_for_schema_named(
                openapi,
                bindings,
                naming,
                raw_item,
                raw_item,
                item_name.clone(),
            )?
        };
        item_models
            .last_mut()
            .ok_or(RESPONSE_VIEW_UNPROVEN)?
            .1
            .borrowed = Some(true);

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
        item_models.push((name.clone(), root_model));
        return Ok((name, item_models));
    }

    let union_items = items.get("oneOf").is_some() || items.get("anyOf").is_some();
    if union_items {
        return Ok((
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
                    type_alias: None,
                    map: None,
                    scalar_enum: None,
                    union_factory: None,
                    borrowed: Some(false),
                    accessors: Some(IndexMap::new()),
                },
            )],
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
    name: String,
) -> Result<(String, ModelDefinition), &'static str> {
    let alias = bindings.aliases.get(raw).ok_or(RESPONSE_VIEW_UNPROVEN)?;
    let syntax = parse_type(alias).map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
    let schema = openapi.schema(raw).map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
    let mut seen_aliases = BTreeSet::from([raw.to_owned()]);
    if !type_matches_schema(schema, &syntax, bindings, &mut seen_aliases)? {
        return Err(RESPONSE_VIEW_UNPROVEN);
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
    name: String,
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
    name: String,
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
    naming: &ModelNaming<'_>,
    raw: &str,
    resource_path: &[String],
    public_name: &str,
) -> Result<(String, ProjectedModels), &'static str> {
    let name = naming.named(
        raw,
        ModelRepresentation::Owned,
        response_model_name(resource_path, public_name),
    )?;
    if bindings.aliases.contains_key(raw) {
        if !naming.public_name_available(&name, bindings) {
            return Err("capability.public_model_name_collision");
        }
        let (name, model) = alias_response_model(openapi, bindings, raw, name)?;
        return Ok((name.clone(), vec![(name, model)]));
    }
    if bindings.structs.contains_key(raw) {
        let schema = openapi.schema(raw).map_err(|_| RESPONSE_VIEW_UNPROVEN)?;
        if schema.get("type").and_then(Value::as_str) == Some("object")
            && schema
                .get("additionalProperties")
                .is_some_and(|additional| *additional != Value::Bool(false))
            && schema
                .get("properties")
                .and_then(Value::as_object)
                .is_none_or(|properties| properties.is_empty())
        {
            if !naming.public_name_available(&name, bindings) {
                return Err("capability.public_model_name_collision");
            }
            let (name, model) = map_response_model(openapi, bindings, raw, name)?;
            return Ok((name.clone(), vec![(name, model)]));
        }
        return response_view(openapi, bindings, naming, raw, resource_path, public_name);
    }
    if bindings.enums.contains_key(raw) {
        if !naming.public_name_available(&name, bindings) {
            return Err("capability.public_model_name_collision");
        }
        let (name, model) = scalar_enum_response_model(openapi, bindings, raw, name)?;
        return Ok((name.clone(), vec![(name, model)]));
    }
    Err(RESPONSE_VIEW_UNPROVEN)
}

fn inline_union_response_model(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    naming: &ModelNaming<'_>,
    schema: &Value,
    raw_union: &str,
    union_name: String,
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

    if !public_model_name_available(&union_name, bindings) {
        return Err("capability.public_model_name_collision");
    }

    let mut models = Vec::new();
    let mut variants = IndexMap::new();
    for (index, ((raw_variant, raw_payload), branch)) in
        mapping.into_iter().zip(branches).enumerate()
    {
        if !object_value_matches(openapi, branch, &raw_payload, bindings) {
            return Err(RESPONSE_UNION_REQUIRED);
        }
        let public_variant = format!("Variant{}", index + 1);
        let branch_name = format!("{union_name}{public_variant}");
        let (adapter, branch_model) = inline_response_view_named(
            openapi,
            bindings,
            naming,
            branch,
            &raw_payload,
            branch_name,
        )
        .map_err(|_| RESPONSE_UNION_REQUIRED)?;
        models.extend(branch_model);
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
    naming: &ModelNaming<'_>,
    schema: &Value,
    raw_union: &str,
    union_name: String,
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
        match inline_union_response_model(
            openapi,
            bindings,
            naming,
            schema,
            raw_union,
            union_name.clone(),
        ) {
            Ok(projected) => return Ok(projected),
            Err(RESPONSE_UNION_REQUIRED)
                if bindings.enums.contains_key(raw_union)
                    && (rust_type_matches_schema(schema, raw_union, bindings)
                        || response_array_union_matches(openapi, schema, raw_union, bindings)) =>
            {
                let name = union_name.clone();
                if !naming.public_name_available(&name, bindings) {
                    return Err("capability.public_model_name_collision");
                }
                let model = ModelDefinition {
                    schema: None,
                    schema_path: None,
                    raw: Some(raw_union.into()),
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
                    accessors: Some(IndexMap::new()),
                };
                return Ok((name.clone(), vec![(name, model)]));
            }
            Err(reason) => return Err(reason),
        }
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

    if !naming.public_name_available(&union_name, bindings) {
        return Err("capability.public_model_name_collision");
    }
    let mut models = Vec::new();
    let mut variants = IndexMap::new();
    let mut public_variants = BTreeSet::new();
    for reference in references {
        if !request_object_matches(openapi, &reference, &reference, bindings) {
            return Err(RESPONSE_UNION_REQUIRED);
        }
        let public_variant = semantic_pascal_identifier(&reference)?;
        if !public_variants.insert(public_variant.clone()) {
            return Err("capability.public_model_name_collision");
        }
        let branch_name = naming.named(
            &reference,
            ModelRepresentation::Owned,
            format!("{union_name}{public_variant}"),
        )?;
        let (adapter, branch_model) =
            response_view_named(openapi, bindings, naming, &reference, branch_name)
                .map_err(|_| RESPONSE_UNION_REQUIRED)?;
        models.extend(branch_model);
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
    naming: &ModelNaming<'_>,
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
            let union_name = naming.named(
                raw,
                ModelRepresentation::Owned,
                response_model_name(resource_path, public_name),
            )?;
            let (name, models) =
                union_response_model(openapi, bindings, naming, resolved, raw_success, union_name)?;
            return Ok(ProjectedResponse::Json { name, models });
        }
        let (name, models) =
            response_model(openapi, bindings, naming, raw, resource_path, public_name)?;
        return Ok(ProjectedResponse::Json { name, models });
    }
    if unconstrained_json_alias_matches(schema, raw_success, bindings)
        || plain_string_json_alias_matches(schema, raw_success, bindings)
    {
        let name = response_model_name(resource_path, public_name);
        if !public_model_name_available(&name, bindings) {
            return Err("capability.public_model_name_collision");
        }
        return Ok(ProjectedResponse::Json {
            name: name.clone(),
            models: vec![(
                name,
                ModelDefinition {
                    schema: None,
                    schema_path: None,
                    raw: Some(raw_success.into()),
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
        });
    }
    if schema.get("oneOf").is_some() || schema.get("anyOf").is_some() {
        let (name, models) = union_response_model(
            openapi,
            bindings,
            naming,
            schema,
            raw_success,
            response_model_name(resource_path, public_name),
        )?;
        return Ok(ProjectedResponse::Json { name, models });
    }
    if schema.get("type").and_then(Value::as_str) == Some("object") {
        let (name, models) = inline_response_view(
            openapi,
            bindings,
            naming,
            schema,
            raw_success,
            resource_path,
            public_name,
        )?;
        return Ok(ProjectedResponse::Json { name, models });
    }
    if schema.get("type").and_then(Value::as_str) == Some("array") {
        let (name, models) = inline_array_response_model(
            openapi,
            bindings,
            naming,
            schema,
            raw_success,
            resource_path,
            public_name,
        )?;
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

fn sse_raw_payload(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    schema_name: &str,
) -> Result<String, &'static str> {
    // Exact source/raw symbol identity is stronger than anonymous shape
    // matching and disambiguates distinct named schemas that intentionally
    // share the same object layout (for example separate stream error types).
    if bindings.structs.contains_key(schema_name)
        && openapi
            .object_schema(schema_name)
            .ok()
            .is_some_and(|schema| object_field_names_match(&schema, schema_name, bindings))
    {
        return Ok(schema_name.to_owned());
    }

    let raw_candidates = bindings
        .structs
        .keys()
        .filter(|raw| scalar_named_object_matches(openapi, schema_name, raw, bindings))
        .cloned()
        .collect::<Vec<_>>();
    if raw_candidates.len() == 1 {
        return Ok(raw_candidates[0].clone());
    }
    Err("capability.event_stream_payload_not_structurally_provable")
}

fn event_stream_projection(
    openapi: &OpenApiIndex,
    bindings: &Bindings,
    naming: &ModelNaming<'_>,
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
    let payload_sets = schemas
        .iter()
        .map(|schema| {
            sse_payload_schema_names(openapi, schema)
                .ok_or("capability.event_stream_payload_not_structurally_provable")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let payloads = payload_sets
        .first()
        .ok_or("capability.event_stream_payload_not_structurally_provable")?;
    if payload_sets.iter().any(|candidate| candidate != payloads) {
        return Err("capability.event_stream_payload_not_structurally_provable");
    }

    let wrapper_fallback = stream_item_model_name(resource_path, public_name);
    if payloads.len() == 1 {
        let raw_item = sse_raw_payload(openapi, bindings, &payloads[0])?;
        let wrapper = naming.named(&payloads[0], ModelRepresentation::Owned, wrapper_fallback)?;
        let (_, wrapper_model) = response_view_for_schema_named(
            openapi,
            bindings,
            naming,
            &payloads[0],
            &raw_item,
            wrapper.clone(),
        )?;
        return Ok(ProjectedResponse::Sse {
            stream: StreamDefinition {
                item: raw_item,
                wrapper: Some(wrapper.clone()),
                type_name: stream_type_name(resource_path, public_name),
                variants: Vec::new(),
            },
            models: wrapper_model,
        });
    }

    let wrapper = wrapper_fallback;
    if !public_model_name_available(&wrapper, bindings) {
        return Err("capability.public_model_name_collision");
    }
    let decoder = format!("__{wrapper}Raw");
    if bindings.structs.contains_key(&decoder)
        || bindings.enums.contains_key(&decoder)
        || bindings.aliases.contains_key(&decoder)
    {
        return Err("capability.public_model_name_collision");
    }

    let mut models = Vec::new();
    let mut variants = Vec::new();
    let mut public_variants = BTreeSet::new();
    for schema_name in payloads {
        let raw = sse_raw_payload(openapi, bindings, schema_name)?;
        let name = semantic_pascal_identifier(schema_name)
            .map_err(|_| "capability.event_stream_payload_not_structurally_provable")?;
        if !public_variants.insert(name.clone()) {
            return Err("capability.public_model_name_collision");
        }
        let branch_wrapper = naming.named(
            schema_name,
            ModelRepresentation::Owned,
            format!("{wrapper}{name}"),
        )?;
        let (_, model) = response_view_for_schema_named(
            openapi,
            bindings,
            naming,
            schema_name,
            &raw,
            branch_wrapper.clone(),
        )?;
        models.extend(model);
        variants.push(StreamVariantDefinition {
            name,
            schema: schema_name.clone(),
            raw,
            wrapper: branch_wrapper,
        });
    }

    Ok(ProjectedResponse::Sse {
        stream: StreamDefinition {
            item: decoder,
            wrapper: Some(wrapper),
            type_name: stream_type_name(resource_path, public_name),
            variants,
        },
        models,
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
            || !matches!(
                discriminator.rust_value_type.as_str(),
                "bool" | "Option<bool>"
            )
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
    naming: &ModelNaming<'_>,
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
                    naming,
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
                naming,
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
            naming,
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
    stable_model_identity: bool,
    public_models: &BTreeMap<String, String>,
) -> ProjectionResult<ProjectedOperation> {
    let mut path: Vec<_> = public_path.split('.').map(str::to_owned).collect();
    let public_name = path.pop().ok_or("surface.invalid_public_path")?;
    if path.is_empty() || public_name.is_empty() {
        return Err("surface.invalid_public_path".into());
    }

    let naming = ModelNaming::new(stable_model_identity, public_models);
    let operation = openapi
        .operation(operation_id)
        .map_err(|_| "openapi.unknown_operation")?;
    let mut request_model = request_model(
        openapi,
        bindings,
        &naming,
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
    let response = response_projection(
        openapi,
        bindings,
        &naming,
        operation,
        binding,
        &path,
        &public_name,
    )?;
    let multipart_filenames = match multipart_filenames_binding(bindings, binding)? {
        Some(_) if request_media == Some(RequestMediaDefinition::MultipartFormData) => Some(true),
        Some(_) => return Err("capability.multipart_filenames_requires_multipart".into()),
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
        model_identities: naming.identities(),
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

fn identities_can_share_name(
    left: &BTreeSet<ModelIdentity>,
    right: &BTreeSet<ModelIdentity>,
) -> bool {
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let combined: BTreeSet<_> = left.iter().chain(right).cloned().collect();
    let representations: BTreeSet<_> = combined
        .iter()
        .map(|identity| identity.representation)
        .collect();
    if representations.len() != 1 {
        return false;
    }
    let sources: BTreeSet<_> = combined
        .iter()
        .map(|identity| identity.source_schema.as_str())
        .collect();
    sources.len() == 1 || combined.iter().all(|identity| identity.explicit)
}

fn identity_description(identities: &BTreeSet<ModelIdentity>) -> String {
    if identities.is_empty() {
        return "anonymous/inline projection".into();
    }
    identities
        .iter()
        .map(|identity| {
            let representation = match identity.representation {
                ModelRepresentation::Owned => "owned",
                ModelRepresentation::Borrowed => "borrowed",
            };
            let policy = if identity.explicit {
                "explicit"
            } else {
                "implicit"
            };
            format!("{} ({representation}, {policy})", identity.source_schema)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn projected_contract_eq(left: &ModelDefinition, right: &ModelDefinition) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.schema = None;
    left.schema_path = None;
    right.schema = None;
    right.schema_path = None;
    left == right
}

fn model_collision(
    name: &str,
    left: &BTreeSet<ModelIdentity>,
    right: &BTreeSet<ModelIdentity>,
    incompatible_definition: bool,
) -> ProjectionFailure {
    let mismatch = if incompatible_definition {
        "project to incompatible public definitions"
    } else {
        "do not share an authoritative public identity"
    };
    ProjectionFailure {
        code: "capability.public_model_identity_collision".into(),
        detail: Some(format!(
            "public model {name} collides: [{}] and [{}] {mismatch}",
            identity_description(left),
            identity_description(right),
        )),
    }
}

pub(crate) fn insert_projection(
    definition: &mut SdkDefinition,
    registry: &mut ProjectionRegistry,
    projected: ProjectedOperation,
) -> ProjectionResult<()> {
    let mut pending: Vec<(String, ModelDefinition, BTreeSet<ModelIdentity>)> = Vec::new();
    for (name, model) in projected.models {
        let identities = projected
            .model_identities
            .get(&name)
            .cloned()
            .unwrap_or_default();
        if let Some((_, existing_model, existing_identities)) = pending
            .iter_mut()
            .find(|(candidate, _, _)| candidate == &name)
        {
            let compatible_identity = identities_can_share_name(existing_identities, &identities);
            let compatible_definition = projected_contract_eq(existing_model, &model);
            if !compatible_identity || !compatible_definition {
                return Err(model_collision(
                    &name,
                    existing_identities,
                    &identities,
                    !compatible_definition,
                ));
            }
            existing_identities.extend(identities);
        } else {
            pending.push((name, model, identities));
        }
    }

    for (name, model, identities) in &pending {
        if let Some(existing) = registry.models.get(name) {
            let compatible_identity = identities_can_share_name(&existing.identities, identities);
            let compatible_definition = projected_contract_eq(&existing.model, model);
            if !compatible_identity || !compatible_definition {
                return Err(model_collision(
                    name,
                    &existing.identities,
                    identities,
                    !compatible_definition,
                ));
            }
        } else if definition.models.contains_key(name) {
            return Err(ProjectionFailure {
                code: "capability.public_model_identity_collision".into(),
                detail: Some(format!(
                    "public model {name} collides with a definition that has no projected source identity"
                )),
            });
        }
    }

    for depth in 1..=projected.resource_path.len() {
        let path = projected.resource_path[..depth].to_vec();
        let module = path.join("_");
        let name = resource_name(&path);
        if name.is_empty() {
            return Err("surface.invalid_public_path".into());
        }
        if let Some(existing) = definition.resources.get(&module)
            && (existing.path.as_ref() != Some(&path) || existing.name != name)
        {
            return Err("surface.resource_name_collision".into());
        }
    }
    let leaf_module = projected.resource_path.join("_");
    if definition
        .resources
        .get(&leaf_module)
        .is_some_and(|resource| resource.operations.contains_key(&projected.public_name))
    {
        return Err("surface.public_path_collision".into());
    }

    for (name, model, identities) in pending {
        if let Some(existing) = registry.models.get_mut(&name) {
            existing.identities.extend(identities);
            continue;
        }
        definition.models.insert(name.clone(), model.clone());
        registry
            .models
            .insert(name, RegisteredProjection { model, identities });
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

#[cfg(test)]
mod model_identity_tests {
    use super::*;
    use crate::contracts::ClientDefinition;

    fn definition() -> SdkDefinition {
        SdkDefinition {
            schema_version: 2,
            client: ClientDefinition {
                name: "Client".into(),
            },
            models: IndexMap::new(),
            resources: IndexMap::new(),
        }
    }

    fn model(source: Option<&str>, raw: &str) -> ModelDefinition {
        ModelDefinition {
            raw: Some(raw.into()),
            schema: source.map(str::to_owned),
            schema_path: None,
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
            accessors: Some(IndexMap::new()),
        }
    }

    fn identity(source: &str, explicit: bool) -> BTreeSet<ModelIdentity> {
        BTreeSet::from([ModelIdentity {
            source_schema: source.into(),
            representation: ModelRepresentation::Owned,
            explicit,
        }])
    }

    fn projected(
        operation_id: &str,
        operation_name: &str,
        model_name: &str,
        model: ModelDefinition,
        identities: BTreeSet<ModelIdentity>,
    ) -> ProjectedOperation {
        ProjectedOperation {
            resource_path: vec!["things".into()],
            public_name: operation_name.into(),
            models: vec![(model_name.into(), model)],
            model_identities: BTreeMap::from([(model_name.into(), identities)]),
            operation: OperationDefinition {
                operation_id: operation_id.into(),
                raw_method: Some(operation_id.into()),
                request: None,
                request_media: None,
                response: Some(model_name.into()),
                response_representation: Some(ResponseRepresentationDefinition::Json),
                empty_response: None,
                binary_response: None,
                stream: None,
                request_overrides: None,
                multipart_filenames: None,
            },
        }
    }

    #[test]
    fn same_named_source_reuses_one_public_model() {
        let mut definition = definition();
        let mut registry = ProjectionRegistry::default();

        insert_projection(
            &mut definition,
            &mut registry,
            projected(
                "read_one",
                "read_one",
                "Shared",
                model(Some("SharedSchema"), "RawShared"),
                identity("SharedSchema", false),
            ),
        )
        .expect("first projection");
        insert_projection(
            &mut definition,
            &mut registry,
            projected(
                "read_two",
                "read_two",
                "Shared",
                model(Some("SharedSchema"), "RawShared"),
                identity("SharedSchema", false),
            ),
        )
        .expect("same source identity must be reusable");

        assert_eq!(definition.models.len(), 1);
        assert_eq!(
            definition.resources["things"].operations["read_one"]
                .response
                .as_deref(),
            Some("Shared")
        );
        assert_eq!(
            definition.resources["things"].operations["read_two"]
                .response
                .as_deref(),
            Some("Shared")
        );
    }

    #[test]
    fn semantic_identity_can_shadow_backend_symbol_but_fallback_cannot() {
        let bindings = Bindings {
            structs: BTreeMap::from([("Message".into(), Vec::new())]),
            symbol_paths: BTreeMap::from([(
                "Message".into(),
                "crate::generated::types::Message".into(),
            )]),
            ..serde_json::from_str(include_str!(
                "../tests/fixtures/derivation-structured-response/rust-bindings.json"
            ))
            .expect("fixture bindings")
        };

        let explicit = BTreeMap::from([("SourceMessage".into(), "Message".into())]);
        let stable = ModelNaming::new(true, &explicit);
        let name = stable
            .named(
                "SourceMessage",
                ModelRepresentation::Owned,
                "Fallback".into(),
            )
            .expect("semantic name");
        assert_eq!(name, "Message");
        assert!(stable.public_name_available(&name, &bindings));

        let legacy_explicit = BTreeMap::new();
        let legacy = ModelNaming::new(false, &legacy_explicit);
        assert!(!legacy.public_name_available("Message", &bindings));
    }

    #[test]
    fn distinct_named_sources_are_not_structurally_deduplicated() {
        let explicit = BTreeMap::new();
        let naming = ModelNaming::new(true, &explicit);
        assert_eq!(
            naming
                .named(
                    "FirstSchema",
                    ModelRepresentation::Owned,
                    "FallbackOne".into(),
                )
                .expect("first identity"),
            "FirstSchema"
        );
        assert_eq!(
            naming
                .named(
                    "SecondSchema",
                    ModelRepresentation::Owned,
                    "FallbackTwo".into(),
                )
                .expect("second identity"),
            "SecondSchema"
        );
    }

    #[test]
    fn anonymous_equal_models_do_not_gain_semantic_identity() {
        let mut definition = definition();
        let mut registry = ProjectionRegistry::default();

        insert_projection(
            &mut definition,
            &mut registry,
            projected(
                "read_one",
                "read_one",
                "Inline",
                model(None, "RawInline"),
                BTreeSet::new(),
            ),
        )
        .expect("first anonymous model");
        let error = insert_projection(
            &mut definition,
            &mut registry,
            projected(
                "read_two",
                "read_two",
                "Inline",
                model(None, "RawInline"),
                BTreeSet::new(),
            ),
        )
        .expect_err("anonymous structure must not imply identity");

        assert_eq!(error.code, "capability.public_model_identity_collision");
        assert!(
            error
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("anonymous/inline projection"))
        );
    }

    #[test]
    fn explicit_policy_can_unify_compatible_source_identities() {
        let mut definition = definition();
        let mut registry = ProjectionRegistry::default();

        insert_projection(
            &mut definition,
            &mut registry,
            projected(
                "read_first",
                "read_first",
                "Concept",
                model(Some("FirstSchema"), "RawShared"),
                identity("FirstSchema", true),
            ),
        )
        .expect("first explicit identity");
        insert_projection(
            &mut definition,
            &mut registry,
            projected(
                "read_second",
                "read_second",
                "Concept",
                model(Some("SecondSchema"), "RawShared"),
                identity("SecondSchema", true),
            ),
        )
        .expect("explicitly unified compatible identity");

        assert_eq!(definition.models.len(), 1);
    }

    #[test]
    fn incompatible_explicit_identity_collision_is_actionable() {
        let mut definition = definition();
        let mut registry = ProjectionRegistry::default();

        insert_projection(
            &mut definition,
            &mut registry,
            projected(
                "read_first",
                "read_first",
                "Concept",
                model(Some("FirstSchema"), "RawFirst"),
                identity("FirstSchema", true),
            ),
        )
        .expect("first explicit identity");
        let error = insert_projection(
            &mut definition,
            &mut registry,
            projected(
                "read_second",
                "read_second",
                "Concept",
                model(Some("SecondSchema"), "RawSecond"),
                identity("SecondSchema", true),
            ),
        )
        .expect_err("different public contracts must not be silently unified");

        assert_eq!(error.code, "capability.public_model_identity_collision");
        let detail = error.detail.expect("actionable collision detail");
        assert!(detail.contains("FirstSchema"));
        assert!(detail.contains("SecondSchema"));
        assert!(detail.contains("incompatible public definitions"));
    }

    #[test]
    fn same_projection_can_be_shared_by_request_and_response() {
        let mut definition = definition();
        let mut registry = ProjectionRegistry::default();

        let mut request = projected(
            "write_shared",
            "write_shared",
            "Shared",
            model(Some("SharedSchema"), "RawShared"),
            identity("SharedSchema", false),
        );
        request.operation.request = Some("Shared".into());
        request.operation.response = None;
        request.operation.response_representation = None;
        insert_projection(&mut definition, &mut registry, request).expect("request projection");

        insert_projection(
            &mut definition,
            &mut registry,
            projected(
                "read_shared",
                "read_shared",
                "Shared",
                model(Some("SharedSchema"), "RawShared"),
                identity("SharedSchema", false),
            ),
        )
        .expect("compatible response projection");

        assert_eq!(definition.models.len(), 1);
        assert_eq!(
            definition.resources["things"].operations["write_shared"]
                .request
                .as_deref(),
            Some("Shared")
        );
        assert_eq!(
            definition.resources["things"].operations["read_shared"]
                .response
                .as_deref(),
            Some("Shared")
        );
    }

    #[test]
    fn ownership_representation_is_deterministic_and_not_operation_derived() {
        let explicit = BTreeMap::from([("SharedSchema".into(), "Shared".into())]);
        let naming = ModelNaming::new(true, &explicit);

        let owned = naming
            .named(
                "SharedSchema",
                ModelRepresentation::Owned,
                "OperationOneResponse".into(),
            )
            .expect("owned identity");
        let borrowed = naming
            .named(
                "SharedSchema",
                ModelRepresentation::Borrowed,
                "OperationTwoNestedValue".into(),
            )
            .expect("borrowed identity");

        assert_eq!(owned, "Shared");
        assert_eq!(borrowed, "SharedRef");
    }
}
