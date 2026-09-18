use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use serde_json::Value;

use crate::contracts::{
    AccessorKindDefinition, Bindings, FieldBinding, ModelDefinition, OpenApi, OperationBinding,
    RequestMediaDefinition, ResponseRepresentationBinding, ResponseRepresentationDefinition,
    SdkDefinition, SimpleUnionVariant,
};
use crate::error::{GenerationError, Result};
use crate::ir::*;
use crate::openapi::{OpenApiIndex, ref_name};
use crate::rust_type::{Type, TypeKind, parse_type};
use crate::structural::{
    inline_object_union_mapping, raw_scalar_struct_shape, request_object_matches,
    request_optional_boolean_field, request_union_mapping, rust_type_matches_schema,
    scalar_object_shape,
};
use crate::symbols::{SymbolProvider, field_identifier};

fn error(code: &'static str, message: impl Into<String>) -> GenerationError {
    GenerationError::new(code, message)
}

fn option(type_name: &str) -> Result<Option<(String, usize)>> {
    let syntax = parse_type(type_name)?;
    let Some(inner) = syntax.unary("Option") else {
        return Ok(None);
    };
    if let Some(nullable) = inner.unary("Option") {
        Ok(Some((nullable.spelling.clone(), 2)))
    } else {
        Ok(Some((inner.spelling.clone(), 1)))
    }
}

fn field_map<'a>(bindings: &'a Bindings, raw: &str) -> Result<IndexMap<String, &'a FieldBinding>> {
    Ok(bindings
        .fields(raw)?
        .iter()
        .map(|field| {
            (
                field
                    .name
                    .strip_prefix("r#")
                    .unwrap_or(&field.name)
                    .to_owned(),
                field,
            )
        })
        .collect())
}

fn argument(name: &str, type_name: &str) -> Result<(ArgumentSpec, ValueSpec)> {
    let public_name = field_identifier(name)?;
    if type_name == "String" {
        Ok((
            ArgumentSpec {
                name: public_name.clone(),
                kind: ArgumentKind::IntoString,
                type_name: "String".into(),
            },
            ValueSpec::IntoString(public_name),
        ))
    } else {
        Ok((
            ArgumentSpec {
                name: public_name.clone(),
                kind: ArgumentKind::Exact,
                type_name: type_name.into(),
            },
            ValueSpec::Variable(public_name),
        ))
    }
}

fn wrap(type_name: &str, value: ValueSpec) -> Result<ValueSpec> {
    Ok(if let Some((_, depth)) = option(type_name)? {
        ValueSpec::Some {
            value: Box::new(value),
            depth,
        }
    } else {
        value
    })
}

fn constructor_argument(
    name: &str,
    field: &FieldBinding,
    adapter: Option<&str>,
) -> Result<(ArgumentSpec, ValueSpec)> {
    let public_name = field_identifier(name)?;
    if let Some(adapter) = adapter {
        if parse_type(&field.type_name)?.unary("Vec").is_some() {
            return Ok((
                ArgumentSpec {
                    name: public_name.clone(),
                    kind: ArgumentKind::IntoIterModel,
                    type_name: adapter.into(),
                },
                ValueSpec::CollectInto(public_name),
            ));
        }
        return Ok((
            ArgumentSpec {
                name: public_name.clone(),
                kind: ArgumentKind::IntoModel,
                type_name: adapter.into(),
            },
            ValueSpec::IntoModel {
                name: public_name,
                adapter: adapter.into(),
            },
        ));
    }
    let effective = option(&field.type_name)?
        .map(|(inner, _)| inner)
        .unwrap_or_else(|| field.type_name.clone());
    let (argument, value) = argument(name, &effective)?;
    Ok((argument, wrap(&field.type_name, value)?))
}

fn struct_value(
    raw: &str,
    fields: &IndexMap<String, &FieldBinding>,
    values: &BTreeMap<String, ValueSpec>,
) -> Result<StructValue> {
    let mut assignments = Vec::new();
    for field in fields.values() {
        let name = field.name.strip_prefix("r#").unwrap_or(&field.name);
        if let Some(value) = values.get(name) {
            let shorthand = matches!(value, ValueSpec::Variable(value_name) if value_name == &field.name && !field.name.starts_with("r#"));
            assignments.push(StructFieldValue {
                name: field.name.clone(),
                value: value.clone(),
                shorthand,
            });
        } else if option(&field.type_name)?.is_some() {
            assignments.push(StructFieldValue {
                name: field.name.clone(),
                value: ValueSpec::Literal("None".into()),
                shorthand: false,
            });
        } else {
            return Err(error(
                "lower.required_field",
                format!("required raw field {raw}.{name} has no value"),
            ));
        }
    }
    Ok(StructValue {
        type_name: raw.into(),
        fields: assignments,
    })
}

fn factory_value(
    type_name: &str,
    argument_name: &str,
    bindings: &Bindings,
) -> Result<(ArgumentSpec, ValueSpec)> {
    let public_name = field_identifier(argument_name)?;
    let core = option(type_name)?
        .map(|(inner, _)| inner)
        .unwrap_or_else(|| type_name.to_owned());
    let (spec, value) = if core == "String" {
        (
            ArgumentSpec {
                name: public_name.clone(),
                kind: ArgumentKind::IntoString,
                type_name: "String".into(),
            },
            ValueSpec::IntoString(public_name.clone()),
        )
    } else if let Some(variants) = bindings.enums.get(&core) {
        if let Some(string) = variants
            .iter()
            .find(|variant| variant.payload.as_deref() == Some("String"))
        {
            (
                ArgumentSpec {
                    name: public_name.clone(),
                    kind: ArgumentKind::IntoString,
                    type_name: "String".into(),
                },
                ValueSpec::Enum {
                    type_name: core.clone(),
                    variant: string.name.clone(),
                    value: Box::new(ValueSpec::IntoString(public_name.clone())),
                },
            )
        } else {
            (
                ArgumentSpec {
                    name: public_name.clone(),
                    kind: ArgumentKind::Exact,
                    type_name: core.clone(),
                },
                ValueSpec::Variable(public_name.clone()),
            )
        }
    } else {
        (
            ArgumentSpec {
                name: public_name.clone(),
                kind: ArgumentKind::Exact,
                type_name: core,
            },
            ValueSpec::Variable(public_name.clone()),
        )
    };
    Ok((spec, wrap(type_name, value)?))
}

fn resolve_wrapper(
    name: &str,
    raw: &str,
    model: &ModelDefinition,
    openapi: &OpenApiIndex,
    bindings: &Bindings,
) -> Result<WrapperModelSpec> {
    let fields = field_map(bindings, raw)?;
    let constructor_fields = model.constructor.clone().unwrap_or_default();
    let adapters = model.adapters.clone().unwrap_or_default();
    let mut constructor = None;
    if model.constructor.is_some() || model.union_factory.is_none() {
        let mut arguments = Vec::new();
        let mut values = BTreeMap::new();
        for field_name in &constructor_fields {
            let field = fields.get(field_name).ok_or_else(|| {
                error(
                    "lower.constructor_field",
                    format!("constructor field {raw}.{field_name} not found"),
                )
            })?;
            let (argument, value) = constructor_argument(
                field_name,
                field,
                adapters.get(field_name).map(String::as_str),
            )?;
            arguments.push(argument);
            values.insert(field_name.clone(), value);
        }
        constructor = Some(ConstructorSpec {
            arguments,
            value: struct_value(raw, &fields, &values)?,
        });
    }

    let mut factories = Vec::new();
    if let Some(factory) = &model.union_factory {
        let union_field = fields.get(&factory.field).ok_or_else(|| {
            error(
                "lower.union_factory_field",
                format!("union factory field {raw}.{} not found", factory.field),
            )
        })?;
        let raw_union = option(&union_field.type_name)?
            .map(|(inner, _)| inner)
            .unwrap_or_else(|| union_field.type_name.clone());
        let (discriminator, mapping) = openapi.union(raw, std::slice::from_ref(&factory.field))?;
        let raw_variants: BTreeMap<_, _> = bindings
            .variants(&raw_union)?
            .iter()
            .filter_map(|variant| {
                variant
                    .payload
                    .as_ref()
                    .map(|payload| (payload.clone(), variant.name.clone()))
            })
            .collect();
        for (tag, payload) in mapping {
            let payload_schema = openapi.schema(&payload)?;
            let candidates: Vec<String> = payload_schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|candidate| *candidate != discriminator)
                .map(str::to_owned)
                .collect();
            if candidates.len() != 1 {
                return Err(error(
                    "lower.union_factory_input",
                    format!(
                        "factory branch {payload} needs one non-discriminator input, got {candidates:?}"
                    ),
                ));
            }
            let input_name = &candidates[0];
            let payload_fields = field_map(bindings, &payload)?;
            let payload_field = payload_fields.get(input_name).ok_or_else(|| {
                error(
                    "lower.union_factory_input",
                    format!("raw field {payload}.{input_name} not found"),
                )
            })?;
            let mut arguments = Vec::new();
            let mut outer_values = BTreeMap::new();
            for leading in &factory.leading {
                let field = fields.get(leading).ok_or_else(|| {
                    error(
                        "lower.union_factory_leading",
                        format!("raw field {raw}.{leading} not found"),
                    )
                })?;
                let (argument, value) = factory_value(&field.type_name, leading, bindings)?;
                arguments.push(argument);
                outer_values.insert(leading.clone(), value);
            }
            let (argument, value) = factory_value(&payload_field.type_name, input_name, bindings)?;
            arguments.push(argument);
            let mut payload_values = BTreeMap::new();
            payload_values.insert(input_name.clone(), value);
            let payload_value =
                ValueSpec::Struct(struct_value(&payload, &payload_fields, &payload_values)?);
            let raw_variant = raw_variants.get(&payload).ok_or_else(|| {
                error(
                    "lower.union_factory_variant",
                    format!("raw union {raw_union} has no branch for {payload}"),
                )
            })?;
            let union_value = ValueSpec::Enum {
                type_name: raw_union.clone(),
                variant: raw_variant.clone(),
                value: Box::new(payload_value),
            };
            outer_values.insert(
                factory.field.clone(),
                wrap(&union_field.type_name, union_value)?,
            );
            factories.push(FactorySpec {
                name: factory
                    .rename
                    .get(&tag)
                    .cloned()
                    .unwrap_or_else(|| tag.replace('-', "_")),
                arguments,
                value: struct_value(raw, &fields, &outer_values)?,
            });
        }
    }

    let excluded: BTreeSet<String> = model
        .exclude
        .clone()
        .unwrap_or_default()
        .into_iter()
        .chain(constructor_fields)
        .collect();
    let mut setters = Vec::new();
    for field in bindings.fields(raw)? {
        let name = field.name.strip_prefix("r#").unwrap_or(&field.name);
        if excluded.contains(name) {
            continue;
        }
        let Some((inner, depth)) = option(&field.type_name)? else {
            return Err(error(
                "lower.required_field_review",
                format!(
                    "new required field {raw}.{name}; constructor policy needs semantic review"
                ),
            ));
        };
        let (argument, value) = if let Some(adapter) = adapters.get(name) {
            let synthetic = FieldBinding {
                name: field.name.clone(),
                wire_name: field.wire_name.clone(),
                type_name: inner.clone(),
            };
            constructor_argument(name, &synthetic, Some(adapter))?
        } else {
            argument(name, &inner)?
        };
        setters.push(SetterSpec {
            name: field_identifier(name)?,
            raw_field: field.name.clone(),
            argument,
            value: ValueSpec::Some {
                value: Box::new(value),
                depth,
            },
            null_name: (depth == 2).then(|| format!("{name}_null")),
        });
    }

    let default = constructor
        .as_ref()
        .is_some_and(|ctor| ctor.arguments.is_empty())
        && model.union_factory.is_none();
    let _ = name;
    Ok(WrapperModelSpec {
        constructor,
        factories,
        setters,
        default,
    })
}

fn public_variant(tag: &str) -> String {
    tag.replace('-', "_")
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn string_payload(
    raw_payload: &str,
    payload_field: &str,
    bindings: &Bindings,
) -> Result<StructValue> {
    let mut assignments = Vec::new();
    for field in bindings.fields(raw_payload)? {
        let name = field.name.strip_prefix("r#").unwrap_or(&field.name);
        if name == payload_field {
            let core = option(&field.type_name)?
                .map(|(inner, _)| inner)
                .unwrap_or_else(|| field.type_name.clone());
            let value = if core == "String" {
                ValueSpec::Variable("content".into())
            } else {
                let variant = bindings
                    .variants(&core)?
                    .iter()
                    .find(|variant| variant.payload.as_deref() == Some("String"))
                    .ok_or_else(|| {
                        error(
                            "lower.union_string_branch",
                            format!("{raw_payload}.{payload_field} has no String branch"),
                        )
                    })?;
                ValueSpec::Enum {
                    type_name: core,
                    variant: variant.name.clone(),
                    value: Box::new(ValueSpec::Variable("content".into())),
                }
            };
            assignments.push(StructFieldValue {
                name: field.name.clone(),
                value: wrap(&field.type_name, value)?,
                shorthand: false,
            });
        } else if option(&field.type_name)?.is_some() {
            assignments.push(StructFieldValue {
                name: field.name.clone(),
                value: ValueSpec::Literal("None".into()),
                shorthand: false,
            });
        } else {
            return Err(error(
                "lower.union_payload",
                format!("{raw_payload} also requires {name}"),
            ));
        }
    }
    Ok(StructValue {
        type_name: raw_payload.into(),
        fields: assignments,
    })
}

fn resolve_union(
    raw: &str,
    model: &ModelDefinition,
    openapi: &OpenApiIndex,
    bindings: &Bindings,
) -> Result<UnionModelSpec> {
    let union = model.union.as_ref().expect("union model");
    let (_, mapping) = openapi.union(&union.root, &union.path)?;
    let mut branches = Vec::new();
    for (tag, raw_payload) in &mapping {
        let public_name = public_variant(tag);
        let (argument_spec, raw_value, public_type) =
            match string_payload(raw_payload, &union.payload, bindings) {
                Ok(value) => (
                    ArgumentSpec {
                        name: "content".into(),
                        kind: ArgumentKind::IntoString,
                        type_name: "String".into(),
                    },
                    ValueSpec::Struct(value),
                    "String".to_owned(),
                ),
                Err(_) => (
                    ArgumentSpec {
                        name: "value".into(),
                        kind: ArgumentKind::Exact,
                        type_name: raw_payload.clone(),
                    },
                    ValueSpec::Variable("value".into()),
                    raw_payload.clone(),
                ),
            };
        branches.push(UnionBranchSpec {
            public_name,
            constructor_name: tag.replace('-', "_"),
            public_type,
            argument: argument_spec,
            raw_payload: raw_payload.clone(),
            raw_value,
        });
    }

    let mut targets = Vec::new();
    for target in std::iter::once(raw).chain(union.targets.iter().map(String::as_str)) {
        let raw_variants: BTreeMap<String, String> = bindings
            .variants(target)?
            .iter()
            .filter_map(|variant| {
                variant
                    .payload
                    .as_ref()
                    .map(|payload| (payload.clone(), variant.name.clone()))
            })
            .collect();
        let variants = mapping
            .values()
            .map(|payload| {
                raw_variants
                    .get(payload)
                    .cloned()
                    .map(|variant| (payload.clone(), variant))
                    .ok_or_else(|| {
                        error(
                            "lower.union_variant",
                            format!("raw union {target} has no branch for {payload}"),
                        )
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        targets.push(UnionTargetSpec {
            raw: target.into(),
            variants,
        });
    }
    Ok(UnionModelSpec { branches, targets })
}

fn expand_alias(syntax: Type, bindings: &Bindings, seen: &mut Vec<String>) -> Result<Type> {
    let Some(alias) = bindings.aliases.get(&syntax.spelling) else {
        return Ok(syntax);
    };
    if seen.contains(&syntax.spelling) {
        return Err(error(
            "lower.recursive_alias",
            format!("recursive raw type alias: {}", syntax.spelling),
        ));
    }
    seen.push(syntax.spelling.clone());
    let expanded = expand_alias(parse_type(alias)?, bindings, seen)?;
    seen.pop();
    Ok(expanded)
}

fn adapted_public_type(
    syntax: Type,
    adapter: &str,
    bindings: &Bindings,
) -> Result<(String, usize)> {
    let syntax = expand_alias(syntax, bindings, &mut Vec::new())?;
    if syntax.constructor.as_deref() == Some("Vec") && syntax.arguments.len() == 1 {
        let (inner, depth) = adapted_public_type(syntax.arguments[0].clone(), adapter, bindings)?;
        Ok((format!("Vec<{inner}>"), depth + 1))
    } else {
        Ok((adapter.into(), 0))
    }
}

fn public_alias_type(syntax: Type, bindings: &Bindings, seen: &mut Vec<String>) -> Result<String> {
    if let Some(alias) = bindings.aliases.get(&syntax.spelling) {
        if seen.contains(&syntax.spelling) {
            return Err(error(
                "lower.recursive_alias",
                format!("recursive raw type alias: {}", syntax.spelling),
            ));
        }
        seen.push(syntax.spelling.clone());
        let rendered = public_alias_type(parse_type(alias)?, bindings, seen)?;
        seen.pop();
        return Ok(rendered);
    }
    if bindings.symbol_paths.contains_key(&syntax.spelling) {
        return Err(error(
            "lower.public_alias_generated",
            format!(
                "public type alias references generated symbol: {}",
                syntax.spelling
            ),
        ));
    }
    if syntax.kind == TypeKind::Generic {
        let arguments = syntax
            .arguments
            .into_iter()
            .map(|argument| public_alias_type(argument, bindings, seen))
            .collect::<Result<Vec<_>>>()?;
        Ok(format!(
            "{}<{}>",
            syntax.constructor.expect("generic constructor"),
            arguments.join(", ")
        ))
    } else {
        Ok(syntax.spelling)
    }
}

fn resolve_simple_union(
    raw: &str,
    model: &ModelDefinition,
    bindings: &Bindings,
) -> Result<SimpleUnionModelSpec> {
    let config = model.simple_union.as_ref().expect("simple union");
    let raw_variants: BTreeMap<_, _> = bindings
        .variants(raw)?
        .iter()
        .map(|variant| (variant.name.clone(), variant))
        .collect();
    let mut branches = Vec::new();
    for (raw_name, configured) in &config.variants {
        let variant = raw_variants.get(raw_name).ok_or_else(|| {
            error(
                "lower.simple_union_variant",
                format!("raw union {raw} has no variant {raw_name}"),
            )
        })?;
        let payload = variant.payload.as_ref().ok_or_else(|| {
            error(
                "lower.simple_union_payload",
                format!("simple union {raw}::{raw_name} has no payload"),
            )
        })?;
        let raw_syntax = expand_alias(parse_type(payload)?, bindings, &mut Vec::new())?;
        let (public_name, adapter) = match configured {
            SimpleUnionVariant::Name(name) => (name.clone(), None),
            SimpleUnionVariant::Adapted { name, adapter } => (name.clone(), Some(adapter.as_str())),
        };
        let (public_type, adapt_depth) = if let Some(adapter) = adapter {
            let (type_name, depth) = adapted_public_type(raw_syntax, adapter, bindings)?;
            (type_name, Some(depth))
        } else {
            (bindings.qualified_type(&raw_syntax.spelling)?, None)
        };
        branches.push(SimpleUnionBranchSpec {
            raw_name: raw_name.clone(),
            public_name,
            public_type,
            adapt_depth,
        });
    }
    Ok(SimpleUnionModelSpec {
        branches,
        bidirectional: config.bidirectional,
    })
}

fn generic_inner(type_name: &str, constructor: &str) -> Result<String> {
    parse_type(type_name)?
        .unary(constructor)
        .map(|inner| inner.spelling.clone())
        .ok_or_else(|| {
            error(
                "lower.expected_generic",
                format!("expected {constructor}, got {type_name}"),
            )
        })
}

fn accessor_type(raw: &str, path: &[String], bindings: &Bindings) -> Result<String> {
    let mut current = raw.to_owned();
    for segment in path {
        let expanded = expand_alias(parse_type(&current)?, bindings, &mut Vec::new())?.spelling;
        current = if segment == "first" {
            generic_inner(&expanded, "Vec")?
        } else if segment == "optional" {
            generic_inner(&expanded, "Option")?
        } else {
            let fields = field_map(bindings, &expanded)?;
            fields
                .get(segment)
                .ok_or_else(|| {
                    error(
                        "lower.accessor_path",
                        format!("accessor path field {expanded}.{segment} not found"),
                    )
                })?
                .type_name
                .clone()
        };
    }
    Ok(expand_alias(parse_type(&current)?, bindings, &mut Vec::new())?.spelling)
}

fn resolve_view(raw: &str, model: &ModelDefinition, bindings: &Bindings) -> Result<ViewModelSpec> {
    let mut accessors = Vec::new();
    for (name, config) in model.accessors.as_ref().expect("view accessors") {
        let result_type = accessor_type(raw, &config.path, bindings)?;
        let (return_type, wrapper, enum_type, enum_variant) = match config.kind {
            AccessorKindDefinition::Copy => (result_type.clone(), None, None, None),
            AccessorKindDefinition::Ref => (
                format!(
                    "&{}",
                    if result_type == "String" {
                        "str"
                    } else {
                        &result_type
                    }
                ),
                None,
                None,
                None,
            ),
            AccessorKindDefinition::OptionalCopy => {
                let inner = generic_inner(&result_type, "Option")?;
                (format!("Option<{inner}>"), None, None, None)
            }
            AccessorKindDefinition::OptionalRef => {
                let inner = generic_inner(&result_type, "Option")?;
                let parsed = parse_type(&inner)?;
                let public = if inner == "String" {
                    "str".into()
                } else if let Some(vector) = parsed.unary("Vec") {
                    format!("[{}]", vector.spelling)
                } else {
                    inner
                };
                (format!("Option<&{public}>"), None, None, None)
            }
            AccessorKindDefinition::Iter => {
                let _ = generic_inner(&result_type, "Vec")?;
                let wrapper = config.wrapper.clone().ok_or_else(|| {
                    error(
                        "lower.iter_wrapper",
                        format!("iter accessor {name} requires a wrapper"),
                    )
                })?;
                (
                    format!("impl ExactSizeIterator<Item = {wrapper}<'_>>"),
                    Some(wrapper),
                    None,
                    None,
                )
            }
            AccessorKindDefinition::FirstStringVariant => {
                let variant = bindings
                    .variants(&result_type)?
                    .iter()
                    .find(|variant| variant.payload.as_deref() == Some("String"))
                    .ok_or_else(|| {
                        error(
                            "lower.accessor_string_variant",
                            format!("accessor {name} target {result_type} has no String branch"),
                        )
                    })?;
                (
                    "Option<&str>".into(),
                    None,
                    Some(result_type.clone()),
                    Some(variant.name.clone()),
                )
            }
        };
        accessors.push(ResolvedAccessor {
            name: name.clone(),
            kind: config.kind.clone(),
            path: config.path.clone(),
            return_type,
            wrapper,
            enum_type,
            enum_variant,
        });
    }
    Ok(ViewModelSpec {
        borrowed: model.borrowed.unwrap_or(true),
        accessors,
    })
}

fn unwrap_nullable_schema(schema: &Value) -> &Value {
    let Some(branches) = schema.get("anyOf").and_then(Value::as_array) else {
        return schema;
    };
    let non_null: Vec<_> = branches
        .iter()
        .filter(|branch| branch.get("type").and_then(Value::as_str) != Some("null"))
        .collect();
    if non_null.len() == 1 && non_null.len() != branches.len() {
        non_null[0]
    } else {
        schema
    }
}

fn schema_at<'a>(openapi: &'a OpenApiIndex, root: &str, path: &[String]) -> Result<&'a Value> {
    let mut schema = openapi.schema(root)?;
    for segment in path {
        schema = unwrap_nullable_schema(schema);
        schema = if segment == "items" {
            schema.get("items")
        } else {
            schema
                .get("properties")
                .and_then(|properties| properties.get(segment))
        }
        .ok_or_else(|| {
            error(
                "lower.schema_path",
                format!("invalid schema path {root}.{}", path.join(".")),
            )
        })?;
    }
    Ok(unwrap_nullable_schema(schema))
}

fn resolve_map(
    raw: &str,
    model: &ModelDefinition,
    openapi: &OpenApiIndex,
    bindings: &Bindings,
) -> Result<MapModelSpec> {
    let config = model.map.as_ref().expect("map policy");
    let schema = schema_at(openapi, &config.root, &config.path)?;
    let additional = schema.get("additionalProperties").ok_or_else(|| {
        error(
            "lower.map_schema",
            format!("map policy does not resolve to additionalProperties"),
        )
    })?;
    if schema.get("type").and_then(Value::as_str) != Some("object")
        || additional == &Value::Bool(false)
    {
        return Err(error(
            "lower.map_schema",
            "map policy does not resolve to additionalProperties",
        ));
    }
    let fields = bindings.fields(raw)?;
    if fields.len() != 1
        || fields[0].name.strip_prefix("r#").unwrap_or(&fields[0].name) != "additional_properties"
    {
        return Err(error(
            "lower.map_wrapper",
            format!("raw map wrapper {raw} must contain only additional_properties"),
        ));
    }
    let mapping = parse_type(&fields[0].type_name)?;
    if mapping.constructor.as_deref() != Some("std::collections::BTreeMap")
        || mapping.arguments.len() != 2
    {
        return Err(error(
            "lower.map_wrapper",
            format!("raw map wrapper {raw}.{} is not a BTreeMap", fields[0].name),
        ));
    }
    if mapping.arguments[0].spelling != "String" {
        return Err(error(
            "lower.map_key",
            format!("raw map wrapper {raw} has non-String keys"),
        ));
    }
    let effective = expand_alias(mapping.arguments[1].clone(), bindings, &mut Vec::new())?;
    if effective.spelling != "serde_json::Value" {
        let additional = unwrap_nullable_schema(additional);
        let expected = match additional.get("type").and_then(Value::as_str) {
            Some("string") => Some("String"),
            Some("integer") => Some("i64"),
            Some("number") => Some("f64"),
            Some("boolean") => Some("bool"),
            _ => None,
        };
        if expected != Some(effective.spelling.as_str()) {
            return Err(error(
                "lower.map_value",
                format!(
                    "raw map value drift for {raw}: {} != {expected:?}",
                    effective.spelling
                ),
            ));
        }
    }
    Ok(MapModelSpec {
        public_type: public_alias_type(mapping, bindings, &mut Vec::new())?,
        raw_field: fields[0].name.clone(),
    })
}

fn resolve_scalar_enum(
    raw: &str,
    model: &ModelDefinition,
    openapi: &OpenApiIndex,
    bindings: &Bindings,
) -> Result<ScalarEnumModelSpec> {
    let config = model.scalar_enum.as_ref().expect("scalar enum policy");
    let schema = schema_at(openapi, &config.root, &config.path)?;
    let values = schema
        .get("enum")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            error(
                "lower.scalar_enum",
                "scalar enum policy does not resolve to a string enum",
            )
        })?;
    if schema.get("type").and_then(Value::as_str) != Some("string")
        || values.is_empty()
        || values.iter().any(|value| !value.is_string())
    {
        return Err(error(
            "lower.scalar_enum",
            "scalar enum policy does not resolve to a string enum",
        ));
    }
    let variants = bindings.variants(raw)?;
    if variants.is_empty()
        || variants
            .iter()
            .any(|variant| variant.payload.is_some() || variant.wire_name.is_none())
    {
        return Err(error(
            "lower.scalar_enum_raw",
            format!("raw scalar enum {raw} requires unit variants with serde rename provenance"),
        ));
    }
    let by_wire: BTreeMap<_, _> = variants
        .iter()
        .map(|variant| {
            (
                variant.wire_name.clone().expect("wire name"),
                variant.name.clone(),
            )
        })
        .collect();
    let expected: BTreeSet<_> = values.iter().filter_map(Value::as_str).collect();
    let actual: BTreeSet<_> = by_wire.keys().map(String::as_str).collect();
    if expected != actual || by_wire.len() != variants.len() {
        return Err(error(
            "lower.scalar_enum_drift",
            format!("raw scalar enum {raw} wire drift"),
        ));
    }
    Ok(ScalarEnumModelSpec {
        variants: values
            .iter()
            .map(|value| by_wire[value.as_str().expect("string enum")].clone())
            .collect(),
    })
}

fn validate_owned_byte_stream(raw_method: &str, success_type: &str) -> Result<()> {
    let transport = parse_type(success_type)?;
    if transport.constructor.as_deref() != Some("futures_util::stream::BoxStream")
        || transport.arguments.len() != 2
    {
        return Err(error(
            "lower.stream_transport",
            format!("raw stream response is not an owned BoxStream: {raw_method}"),
        ));
    }
    let lifetime = &transport.arguments[0];
    let event = &transport.arguments[1];
    if lifetime.spelling != "'static"
        || event.constructor.as_deref() != Some("Result")
        || event.arguments.len() != 2
    {
        return Err(error(
            "lower.stream_ownership",
            format!("raw stream response ownership/item drift: {raw_method}"),
        ));
    }
    if event.arguments[0].spelling != "bytes::Bytes" {
        return Err(error(
            "lower.stream_bytes",
            format!("raw stream response does not yield bytes: {raw_method}"),
        ));
    }
    Ok(())
}

fn success_response<'a>(operation: &'a Value) -> Result<&'a Value> {
    let responses = operation
        .get("responses")
        .and_then(Value::as_object)
        .ok_or_else(|| error("lower.responses", "operation has no responses"))?;
    let mut successful: Vec<_> = responses
        .iter()
        .filter(|(status, _)| status.starts_with('2'))
        .collect();
    successful.sort_by_key(|(status, _)| *status);
    successful
        .first()
        .map(|(_, response)| *response)
        .ok_or_else(|| error("lower.responses", "operation has no successful response"))
}

fn success_schema<'a>(operation: &'a Value, media: &str) -> Result<&'a Value> {
    let responses = operation
        .get("responses")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            error(
                "lower.responses",
                format!("operation has no successful {media} response"),
            )
        })?;
    let mut statuses: Vec<_> = responses.keys().collect();
    statuses.sort();
    for status in statuses {
        if status.starts_with('2') {
            if let Some(schema) = responses[status]
                .get("content")
                .and_then(|content| content.get(media))
                .and_then(|payload| payload.get("schema"))
            {
                return Ok(schema);
            }
        }
    }
    Err(error(
        "lower.responses",
        format!("operation has no successful {media} response"),
    ))
}

fn selected_response_schemas<'a>(
    operation: &'a Value,
    statuses: &[String],
    media_type: &str,
) -> Result<Vec<&'a Value>> {
    let responses = operation
        .get("responses")
        .and_then(Value::as_object)
        .ok_or_else(|| error("lower.responses", "operation has no responses"))?;
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
        return Err(error(
            "lower.responses",
            "operation has no selected success responses",
        ));
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
                .ok_or_else(|| {
                    error(
                        "lower.response_representation_drift",
                        format!("missing selected {media_type} response at status {status}"),
                    )
                })
        })
        .collect()
}

fn selected_responses_are_empty(operation: &Value, statuses: &[String]) -> bool {
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

fn json_schema_for_binding<'a>(
    operation: &'a Value,
    binding: &OperationBinding,
) -> Result<&'a Value> {
    if let Some(metadata) = &binding.metadata {
        let ResponseRepresentationBinding::Json {
            schema_name,
            media_type,
        } = &metadata.representation
        else {
            return Err(error(
                "lower.response_representation_drift",
                "raw binding is not a JSON response representation",
            ));
        };
        if binding.success_type != *schema_name {
            return Err(error(
                "lower.response_representation_drift",
                "raw JSON success type disagrees with canonical representation",
            ));
        }
        let schemas = selected_response_schemas(operation, &metadata.success_statuses, media_type)?;
        let schema = schemas[0];
        if schemas.iter().any(|candidate| *candidate != schema) {
            return Err(error(
                "lower.response_representation_drift",
                "selected JSON response schemas disagree across statuses",
            ));
        }
        return Ok(schema);
    }
    success_schema(operation, "application/json")
}

fn empty_response_matches(operation: &Value, binding: &OperationBinding) -> Result<bool> {
    if let Some(metadata) = &binding.metadata {
        return Ok(matches!(
            metadata.representation,
            ResponseRepresentationBinding::Empty
        ) && binding.success_type == "()"
            && selected_responses_are_empty(operation, &metadata.success_statuses));
    }
    let response = success_response(operation)?;
    Ok(response
        .get("content")
        .is_none_or(|content| content.as_object().is_none_or(|object| object.is_empty()))
        && binding.success_type == "()")
}

fn text_response_schema(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("string")
        && schema.get("format").is_none()
}

fn buffered_binary_response_schema(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("string")
        && schema.get("format").and_then(Value::as_str) == Some("binary")
}

fn buffered_scalar_response_projection(
    operation: &Value,
    binding: &OperationBinding,
    representation: ResponseRepresentationDefinition,
    bindings: &Bindings,
) -> Result<ResponseProjection> {
    let metadata = binding.metadata.as_ref().ok_or_else(|| {
        error(
            "lower.response_representation_drift",
            "buffered scalar response requires canonical binding metadata",
        )
    })?;
    match (representation, &metadata.representation) {
        (
            ResponseRepresentationDefinition::Text,
            ResponseRepresentationBinding::Text { media_type },
        ) => {
            let schemas =
                selected_response_schemas(operation, &metadata.success_statuses, media_type)?;
            if binding.success_type != "String"
                || !schemas.iter().all(|schema| text_response_schema(schema))
            {
                return Err(error(
                    "lower.response_representation_drift",
                    "text response disagrees with canonical binding metadata",
                ));
            }
            Ok(ResponseProjection::Text)
        }
        (
            ResponseRepresentationDefinition::BinaryBuffered,
            ResponseRepresentationBinding::BinaryBuffered { media_type, .. },
        ) => {
            let schemas =
                selected_response_schemas(operation, &metadata.success_statuses, media_type)?;
            if binding.stream.is_some()
                || !matches!(binding.success_type.as_str(), "bytes::Bytes" | "Vec<u8>")
                || !schemas
                    .iter()
                    .all(|schema| buffered_binary_response_schema(schema))
            {
                return Err(error(
                    "lower.response_representation_drift",
                    "buffered binary response disagrees with canonical binding metadata",
                ));
            }
            Ok(ResponseProjection::BinaryBuffered {
                type_name: bindings.qualified_type(&binding.success_type)?,
            })
        }
        _ => Err(error(
            "lower.response_representation_drift",
            "response representation disagrees with canonical binding metadata",
        )),
    }
}

fn response_matches(
    openapi: &OpenApiIndex,
    operation_id: &str,
    raw: &str,
    binding: &OperationBinding,
    bindings: &Bindings,
) -> Result<bool> {
    let operation = openapi.operation(operation_id)?;
    let schema = json_schema_for_binding(operation, binding)?;
    if let Some(referenced) = ref_name(schema) {
        return Ok(referenced == raw);
    }
    let branches = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let payloads: BTreeSet<_> = branches.iter().filter_map(ref_name).collect();
    if !branches.is_empty() && payloads.len() == branches.len() && bindings.enums.contains_key(raw)
    {
        let actual: BTreeSet<_> = bindings
            .variants(raw)?
            .iter()
            .filter_map(|variant| variant.payload.as_deref())
            .collect();
        return Ok(payloads == actual);
    }
    if !branches.is_empty() && inline_object_union_mapping(schema, raw, bindings).is_some() {
        return Ok(true);
    }
    if let Some(wire) = scalar_object_shape(schema) {
        return Ok(raw_scalar_struct_shape(bindings, raw).is_some_and(|actual| actual == wire));
    }
    if schema.get("type").and_then(Value::as_str) == Some("array")
        && bindings.aliases.contains_key(raw)
    {
        return Ok(rust_type_matches_schema(schema, raw, bindings));
    }
    Ok(false)
}

fn request_name(resource: &ResourceSpec, operation_name: &str) -> String {
    let prefix: String = operation_name
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect();
    format!("{prefix}{}Request", resource.name)
}

fn owned_parameter(type_name: &str) -> Result<(String, bool)> {
    let optional = option(type_name)?;
    let inner = optional
        .as_ref()
        .map(|(inner, _)| inner.as_str())
        .unwrap_or(type_name);
    if matches!(inner, "impl AsRef<str>" | "&str") {
        return Ok((
            if optional.is_some() {
                "Option<String>".into()
            } else {
                "String".into()
            },
            true,
        ));
    }
    if inner.contains("impl ") || inner.contains('&') {
        return Err(error(
            "lower.owned_parameter",
            format!("unsupported owned parameter projection: {type_name}"),
        ));
    }
    Ok((type_name.into(), false))
}

fn direct_parameter(parameter: &RawParameter, bindings: &Bindings) -> Result<(String, String)> {
    let name = field_identifier(&parameter.name)?;
    if parameter.type_name == "impl AsRef<str>" {
        return Ok((
            format!("{name}: impl AsRef<str>"),
            format!("{name}.as_ref()"),
        ));
    }
    if parameter.type_name == "&str" {
        return Ok((format!("{name}: &str"), name));
    }
    let (owned, _) = owned_parameter(&parameter.type_name)?;
    let qualified = bindings.qualified_type(&owned)?;
    if qualified == "String" {
        Ok((
            format!("{name}: impl Into<String>"),
            format!("{name}.into()"),
        ))
    } else {
        Ok((format!("{name}: {qualified}"), name))
    }
}

fn parameter_request(
    resource: &ResourceSpec,
    operation: &OperationSpec,
    bindings: &Bindings,
) -> Result<Option<ParameterRequestSpec>> {
    if matches!(
        operation.request_projection,
        RequestProjection::Model { .. } | RequestProjection::Raw { .. }
    ) || operation.raw_signature.parameters.is_empty()
        || !operation
            .raw_signature
            .parameters
            .iter()
            .any(|parameter| option(&parameter.type_name).ok().flatten().is_some())
    {
        return Ok(None);
    }
    let mut fields = Vec::new();
    for parameter in &operation.raw_signature.parameters {
        let (owned, _) = owned_parameter(&parameter.type_name)?;
        let owned = bindings.qualified_type(&owned)?;
        let optional = option(&owned)?;
        if let Some((inner, _)) = optional {
            let public = field_identifier(&parameter.name)?;
            let (setter_argument, setter_value) = if inner == "String" {
                (
                    format!("{public}: impl Into<String>"),
                    format!("{public}.into()"),
                )
            } else {
                (format!("{public}: {inner}"), public.clone())
            };
            fields.push(ParameterField {
                name: parameter.name.clone(),
                type_name: owned,
                constructor_argument: None,
                constructor_value: None,
                setter_argument: Some(setter_argument),
                setter_value: Some(setter_value),
            });
        } else {
            let public = field_identifier(&parameter.name)?;
            let (constructor_argument, constructor_value) = if owned == "String" {
                (
                    format!("{public}: impl Into<String>"),
                    format!("{public}.into()"),
                )
            } else {
                (format!("{public}: {owned}"), public)
            };
            fields.push(ParameterField {
                name: parameter.name.clone(),
                type_name: owned,
                constructor_argument: Some(constructor_argument),
                constructor_value: Some(constructor_value),
                setter_argument: None,
                setter_value: None,
            });
        }
    }
    Ok(Some(ParameterRequestSpec {
        name: request_name(resource, &operation.name),
        fields,
    }))
}

fn operation_call(
    operation: &OperationSpec,
    resource: &ResourceSpec,
    bindings: &Bindings,
) -> Result<OperationCall> {
    let parameters = &operation.raw_signature.parameters;
    if let RequestProjection::Model {
        media: _,
        model,
        raw,
        overrides,
    } = &operation.request_projection
    {
        let mut body = "request.into_raw()".to_owned();
        if !overrides.is_empty() {
            let assignments = overrides
                .iter()
                .map(|(field, configured)| {
                    let value = match configured {
                        Some(true) => "Some(true)",
                        Some(false) => "Some(false)",
                        None => "None",
                    };
                    format!("raw.{field} = {value};")
                })
                .collect::<Vec<_>>()
                .join(" ");
            body = format!("{{ let mut raw = request.into_raw(); {assignments} raw }}");
        }
        let mut declarations = Vec::new();
        let mut values = Vec::new();
        for parameter in parameters {
            if parameter.type_name == *raw {
                declarations.push(format!("request: {model}"));
                values.push(body.clone());
            } else {
                let (declaration, value) = direct_parameter(parameter, bindings)?;
                declarations.push(declaration);
                values.push(value);
            }
        }
        return Ok(OperationCall {
            arguments: declarations.join(", "),
            raw_arguments: values.join(", "),
            default_raw_arguments: None,
        });
    }
    if let RequestProjection::Raw {
        media: _,
        raw_parameter,
        public_name,
    } = &operation.request_projection
    {
        let mut declarations = Vec::new();
        let mut values = Vec::new();
        for parameter in parameters {
            if parameter.name == *raw_parameter {
                let public = RawParameter {
                    name: public_name.clone(),
                    type_name: parameter.type_name.clone(),
                };
                let (declaration, value) = direct_parameter(&public, bindings)?;
                declarations.push(declaration);
                values.push(value);
            } else {
                let (declaration, value) = direct_parameter(parameter, bindings)?;
                declarations.push(declaration);
                values.push(value);
            }
        }
        return Ok(OperationCall {
            arguments: declarations.join(", "),
            raw_arguments: values.join(", "),
            default_raw_arguments: None,
        });
    }
    if parameters.is_empty() {
        return Ok(OperationCall {
            arguments: String::new(),
            raw_arguments: String::new(),
            default_raw_arguments: None,
        });
    }
    let has_optional = parameters
        .iter()
        .any(|parameter| option(&parameter.type_name).ok().flatten().is_some());
    if !has_optional {
        let (declarations, values): (Vec<_>, Vec<_>) = parameters
            .iter()
            .map(|parameter| direct_parameter(parameter, bindings))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .unzip();
        return Ok(OperationCall {
            arguments: declarations.join(", "),
            raw_arguments: values.join(", "),
            default_raw_arguments: None,
        });
    }
    let mut values = Vec::new();
    for parameter in parameters {
        let (_, borrowed) = owned_parameter(&parameter.type_name)?;
        let mut value = format!("request.{}", parameter.name);
        if borrowed {
            value.push_str(if option(&parameter.type_name)?.is_some() {
                ".as_deref()"
            } else {
                ".as_str()"
            });
        }
        values.push(value);
    }
    let all_optional = parameters
        .iter()
        .all(|parameter| option(&parameter.type_name).ok().flatten().is_some());
    let default_raw_arguments = all_optional.then(|| {
        parameters
            .iter()
            .map(|parameter| {
                if parameter.type_name == "Option<impl AsRef<str>>" {
                    "None::<&str>"
                } else {
                    "None"
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    });
    Ok(OperationCall {
        arguments: format!("request: {}", request_name(resource, &operation.name)),
        raw_arguments: values.join(", "),
        default_raw_arguments,
    })
}

fn validate_symbols(ir: &FacadeIr, bindings: &Bindings) -> Result<()> {
    let mut symbols = SymbolProvider::default();
    symbols.claim(&ir.client_name, "sdk", "client", "")?;
    for reserved in ["new", "raw", "with_base_url"] {
        symbols.claim(reserved, &ir.client_name, "client runtime", "")?;
    }
    for model in &ir.models {
        symbols.claim(
            &model.name,
            "sdk",
            &format!("model {}", model.raw),
            "facade_types",
        )?;
        if bindings.structs.contains_key(&model.name) || bindings.enums.contains_key(&model.name) {
            return Err(error(
                "symbol.shadow_raw",
                format!(
                    "facade model {} shadows imported raw type; choose an explicit semantic name",
                    model.name
                ),
            ));
        }
    }
    symbols.claim("facade_types", "modules", "compiler", "")?;
    symbols.claim("mod_file", "modules", "compiler", "")?;
    let by_path: BTreeMap<Vec<String>, &ResourceSpec> = ir
        .resources
        .iter()
        .map(|resource| (resource.path.clone(), resource))
        .collect();
    for resource in &ir.resources {
        if resource.module == "mod" {
            return Err(error(
                "symbol.reserved_module",
                "resource module mod is reserved",
            ));
        }
        symbols.claim(&resource.module, "modules", "resource", "")?;
        symbols.claim(&resource.name, "sdk", "resource", &resource.module)?;
        symbols.claim("new", &resource.name, "resource constructor", "")?;
        if resource.path.len() == 1 {
            symbols.claim(&resource.path[0], &ir.client_name, "resource accessor", "")?;
        } else {
            let parent = by_path
                .get(&resource.path[..resource.path.len() - 1])
                .ok_or_else(|| {
                    error(
                        "symbol.resource_parent",
                        format!("resource {} has no parent", resource.path.join(".")),
                    )
                })?;
            symbols.claim(
                resource.path.last().expect("non-empty path"),
                &parent.name,
                "child resource accessor",
                "",
            )?;
        }
        for operation in &resource.operations {
            symbols.claim(&operation.name, &resource.name, &operation.operation_id, "")?;
            if !matches!(
                operation.request_projection,
                RequestProjection::Model { .. } | RequestProjection::Raw { .. }
            ) && !operation.raw_signature.parameters.is_empty()
                && operation
                    .raw_signature
                    .parameters
                    .iter()
                    .any(|parameter| option(&parameter.type_name).ok().flatten().is_some())
            {
                symbols.claim(
                    &request_name(resource, &operation.name),
                    &format!("module:{}", resource.module),
                    &operation.operation_id,
                    "",
                )?;
                if operation
                    .raw_signature
                    .parameters
                    .iter()
                    .all(|parameter| option(&parameter.type_name).ok().flatten().is_some())
                {
                    symbols.claim(
                        &format!("{}_with", operation.name),
                        &resource.name,
                        &operation.operation_id,
                        "",
                    )?;
                }
            }
            if let ResponseProjection::Sse(stream) = &operation.response_projection {
                symbols.claim(
                    &stream.type_name,
                    "sdk",
                    &operation.operation_id,
                    "facade_types",
                )?;
            }
        }
    }
    Ok(())
}

fn validate_runtime(ir: &FacadeIr, runtime: &crate::Runtime) -> Result<()> {
    let mut public = BTreeSet::new();
    public.insert(ir.client_name.as_str());
    public.extend(ir.models.iter().map(|model| model.name.as_str()));
    public.extend(ir.resources.iter().map(|resource| resource.name.as_str()));
    let exported: BTreeSet<_> = runtime
        .error_exports
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(runtime.error_type.as_str()))
        .collect();
    let collisions: Vec<_> = public.intersection(&exported).copied().collect();
    if !collisions.is_empty() {
        return Err(error(
            "runtime.symbol_collision",
            format!("facade symbols collide with runtime exports: {collisions:?}"),
        ));
    }
    if runtime.error_module == "mod"
        || runtime.error_module == "facade_types"
        || ir
            .resources
            .iter()
            .any(|resource| resource.module == runtime.error_module)
    {
        return Err(error(
            "runtime.module_collision",
            format!(
                "runtime error module collides with generated module: {}",
                runtime.error_module
            ),
        ));
    }
    Ok(())
}

pub(crate) fn lower(
    openapi: &OpenApi,
    bindings: &Bindings,
    definition: &SdkDefinition,
    runtime: &crate::Runtime,
) -> Result<FacadeIr> {
    bindings.validate()?;
    definition.validate()?;
    let index = OpenApiIndex::new(openapi)?;

    let mut models = Vec::new();
    for (name, config) in &definition.models {
        let raw = config.raw.clone().unwrap_or_else(|| name.clone());
        let render = if config.union.is_some() {
            let union = config.union.as_ref().expect("union");
            let (_, mapping) = index.union(&union.root, &union.path)?;
            for target in
                std::iter::once(raw.as_str()).chain(union.targets.iter().map(String::as_str))
            {
                let actual: BTreeSet<_> = bindings
                    .variants(target)?
                    .iter()
                    .filter_map(|variant| variant.payload.as_ref())
                    .collect();
                let expected: BTreeSet<_> = mapping.values().collect();
                if actual != expected {
                    return Err(error(
                        "lower.union_drift",
                        format!("raw union {target} branch drift"),
                    ));
                }
            }
            ModelRenderSpec::Union(resolve_union(&raw, config, &index, bindings)?)
        } else if let Some(simple) = &config.simple_union {
            let configured: BTreeSet<_> = simple.variants.keys().collect();
            let actual: BTreeSet<_> = bindings
                .variants(&raw)?
                .iter()
                .map(|variant| &variant.name)
                .collect();
            if configured != actual {
                return Err(error(
                    "lower.simple_union_drift",
                    format!("raw union {raw} variant drift"),
                ));
            }
            if let Some(root) = config.schema.as_deref() {
                let path = config.schema_path.as_deref().unwrap_or(&[]);
                let schema = schema_at(&index, root, path)?;
                let mapping =
                    request_union_mapping(&index, schema, &raw, bindings).ok_or_else(|| {
                        error(
                            "lower.request_union_drift",
                            format!("OpenAPI/raw request union drift for {raw}"),
                        )
                    })?;
                let proven: BTreeSet<_> =
                    mapping.iter().map(|branch| &branch.raw_variant).collect();
                if configured != proven {
                    return Err(error(
                        "lower.request_union_drift",
                        format!("OpenAPI/raw request union drift for {raw}"),
                    ));
                }
            }
            ModelRenderSpec::SimpleUnion(resolve_simple_union(&raw, config, bindings)?)
        } else if config.type_alias == Some(true) {
            let syntax = bindings.parsed_alias(&raw)?;
            ModelRenderSpec::Alias(AliasModelSpec {
                public_type: public_alias_type(syntax, bindings, &mut vec![raw.clone()])?,
            })
        } else if config.map.is_some() {
            ModelRenderSpec::Map(resolve_map(&raw, config, &index, bindings)?)
        } else if config.scalar_enum.is_some() {
            ModelRenderSpec::ScalarEnum(resolve_scalar_enum(&raw, config, &index, bindings)?)
        } else if config.accessors.is_some() {
            if !bindings.aliases.contains_key(&raw) {
                let _ = bindings.fields(&raw)?;
            }
            ModelRenderSpec::View(resolve_view(&raw, config, bindings)?)
        } else {
            let schema_name = config.schema.as_deref().unwrap_or(&raw);
            let wire_schema = if let Some(path) = config.schema_path.as_deref() {
                index.object_schema_path(schema_name, path)?
            } else {
                index.object_schema(schema_name)?
            };
            let wire_fields: BTreeSet<_> = wire_schema
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|properties| properties.keys())
                .collect();
            let raw_fields: BTreeSet<_> = bindings
                .fields(&raw)?
                .iter()
                .map(|field| {
                    field
                        .name
                        .strip_prefix("r#")
                        .unwrap_or(&field.name)
                        .to_owned()
                })
                .collect();
            if wire_fields
                .iter()
                .map(|name| name.as_str())
                .collect::<BTreeSet<_>>()
                != raw_fields
                    .iter()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>()
            {
                return Err(error(
                    "lower.field_drift",
                    format!("OpenAPI/raw field drift for {raw}"),
                ));
            }
            let mut covered: BTreeSet<String> = config
                .constructor
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect();
            covered.extend(config.exclude.clone().unwrap_or_default());
            if let Some(factory) = &config.union_factory {
                covered.insert(factory.field.clone());
                covered.extend(factory.leading.clone());
            }
            let required: BTreeSet<_> = wire_schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let uncovered: Vec<_> = required
                .into_iter()
                .filter(|field| !covered.contains(*field))
                .collect();
            if !uncovered.is_empty() {
                return Err(error(
                    "lower.required_policy",
                    format!("required fields need constructor policy for {raw}: {uncovered:?}"),
                ));
            }
            ModelRenderSpec::Wrapper(resolve_wrapper(name, &raw, config, &index, bindings)?)
        };
        models.push(ModelSpec {
            name: name.clone(),
            raw,
            render,
        });
    }

    let model_names: BTreeSet<_> = models.iter().map(|model| model.name.as_str()).collect();
    for (name, config) in &definition.models {
        let mut references = Vec::new();
        references.extend(
            config
                .adapters
                .as_ref()
                .into_iter()
                .flat_map(|items| items.values().map(String::as_str)),
        );
        if let Some(simple) = &config.simple_union {
            references.extend(simple.variants.values().filter_map(|value| match value {
                SimpleUnionVariant::Adapted { adapter, .. } => Some(adapter.as_str()),
                SimpleUnionVariant::Name(_) => None,
            }));
        }
        references.extend(config.accessors.as_ref().into_iter().flat_map(|items| {
            items
                .values()
                .filter_map(|accessor| accessor.wrapper.as_deref())
        }));
        let unknown: Vec<_> = references
            .into_iter()
            .filter(|reference| !model_names.contains(reference))
            .collect();
        if !unknown.is_empty() {
            return Err(error(
                "lower.unknown_model_reference",
                format!("model {name} references unknown facade models: {unknown:?}"),
            ));
        }
    }

    for model in &models {
        let ModelRenderSpec::View(view) = &model.render else {
            continue;
        };
        for accessor in &view.accessors {
            if accessor.kind != AccessorKindDefinition::Iter {
                continue;
            }
            let wrapper = accessor
                .wrapper
                .as_deref()
                .expect("validated iter accessor wrapper");
            let raw_collection = accessor_type(&model.raw, &accessor.path, bindings)?;
            let raw_item = generic_inner(&raw_collection, "Vec")?;
            let wrapper_model = models
                .iter()
                .find(|candidate| candidate.name == wrapper)
                .expect("validated model reference");
            if wrapper_model.raw != raw_item
                || !matches!(&wrapper_model.render, ModelRenderSpec::View(item) if item.borrowed)
            {
                return Err(error(
                    "lower.iter_wrapper",
                    format!(
                        "iter accessor {}.{} requires a borrowed wrapper over {raw_item}",
                        model.name, accessor.name
                    ),
                ));
            }
        }
    }

    let mut resources = Vec::new();
    for (module, resource_config) in &definition.resources {
        let path = resource_config
            .path
            .clone()
            .unwrap_or_else(|| vec![module.clone()]);
        let mut resource = ResourceSpec {
            path,
            module: module.clone(),
            name: resource_config.name.clone(),
            operations: Vec::new(),
        };
        for (public_name, item) in &resource_config.operations {
            let operation_id = &item.operation_id;
            let raw_method = item.raw_method.as_ref().unwrap_or(operation_id);
            let raw_operation = bindings.operation(raw_method)?;
            let wire_operation = index.operation(operation_id)?;
            let request = item.request.as_deref();
            let response = item.response.as_deref();

            for referenced in [request, response].into_iter().flatten() {
                if !model_names.contains(referenced) {
                    return Err(error(
                        "lower.unknown_model",
                        format!("unknown facade model {referenced}"),
                    ));
                }
            }

            let wire_parameter_names: Vec<_> = wire_operation
                .get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|parameter| parameter.get("name").and_then(Value::as_str))
                .map(|name| name.replace('-', "_"))
                .collect();
            let is_wire_parameter = |parameter: &crate::ParameterBinding| {
                let name = parameter.name.strip_prefix("r#").unwrap_or(&parameter.name);
                wire_parameter_names.iter().any(|wire| wire == name)
            };

            let mut request_raw_parameter = None;
            let mut raw_request_public_name = None;
            let request_raw = if let Some(request) = request {
                let model = models
                    .iter()
                    .find(|model| model.name == request)
                    .expect("known model");
                let model_definition = &definition.models[request];
                let schema_name = model_definition
                    .schema
                    .as_deref()
                    .unwrap_or(model.raw.as_str());
                let configured_media = item.request_media.unwrap_or(RequestMediaDefinition::Json);
                let body = index
                    .structured_request_body(operation_id)?
                    .ok_or_else(|| {
                        error(
                            "lower.request_media_drift",
                            format!("structured request media drift for {operation_id}"),
                        )
                    })?;
                if body.media != configured_media {
                    return Err(error(
                        "lower.request_media_drift",
                        format!("structured request media drift for {operation_id}"),
                    ));
                }
                let request_matches = if model_definition.schema.is_some() {
                    body.schema == schema_name
                        && request_object_matches(&index, schema_name, &model.raw, bindings)
                } else {
                    body.schema == model.raw
                };
                if !request_matches {
                    return Err(error(
                        "lower.request_drift",
                        format!("OpenAPI request drift for {operation_id}"),
                    ));
                }
                let body_parameters: Vec<_> = raw_operation
                    .parameters
                    .iter()
                    .filter(|parameter| !is_wire_parameter(parameter))
                    .filter(|parameter| parameter.type_name == model.raw)
                    .collect();
                if body_parameters.len() != 1 {
                    return Err(error(
                        "lower.signature_drift",
                        format!("raw signature drift for {raw_method}"),
                    ));
                }
                request_raw_parameter = Some(body_parameters[0].name.clone());
                if let Some(overrides) = &item.request_overrides {
                    for field in overrides.keys() {
                        if !request_optional_boolean_field(
                            &index,
                            schema_name,
                            &model.raw,
                            field,
                            bindings,
                        ) {
                            return Err(error(
                                "lower.request_override",
                                format!(
                                    "request override requires optional Boolean in both contracts: {}.{field}",
                                    model.raw
                                ),
                            ));
                        }
                    }
                }
                Some(model.raw.clone())
            } else if let Some(configured_media) = item.request_media.filter(|media| {
                matches!(
                    media,
                    RequestMediaDefinition::OctetStream
                        | RequestMediaDefinition::Binary
                        | RequestMediaDefinition::TextPlain
                )
            }) {
                let body = index.raw_request_body(operation_id)?.ok_or_else(|| {
                    error(
                        "lower.request_media_drift",
                        format!("raw request media drift for {operation_id}"),
                    )
                })?;
                if body.media != configured_media {
                    return Err(error(
                        "lower.request_media_drift",
                        format!("raw request media drift for {operation_id}"),
                    ));
                }
                let body_parameters: Vec<_> = raw_operation
                    .parameters
                    .iter()
                    .filter(|parameter| !is_wire_parameter(parameter))
                    .filter(|parameter| parameter.type_name == body.type_name)
                    .collect();
                if body_parameters.len() != 1 {
                    return Err(error(
                        "lower.signature_drift",
                        format!("raw body signature drift for {raw_method}"),
                    ));
                }
                let raw_parameter = body_parameters[0];
                request_raw_parameter = Some(raw_parameter.name.clone());

                let used: BTreeSet<_> = raw_operation
                    .parameters
                    .iter()
                    .filter(|parameter| parameter.name != raw_parameter.name)
                    .map(|parameter| {
                        parameter
                            .name
                            .strip_prefix("r#")
                            .unwrap_or(&parameter.name)
                            .to_owned()
                    })
                    .collect();
                let mut public = "body".to_owned();
                let mut suffix = 2;
                if used.contains(&public) {
                    public = "request_body".to_owned();
                    while used.contains(&public) {
                        public = format!("request_body_{suffix}");
                        suffix += 1;
                    }
                }
                raw_request_public_name = Some(public);
                Some(body.type_name)
            } else {
                None
            };

            let raw_parameters: Vec<_> = raw_operation
                .parameters
                .iter()
                .filter(|parameter| {
                    request_raw_parameter.as_deref() != Some(parameter.name.as_str())
                })
                .collect();
            let raw_parameter_names: Vec<_> = raw_parameters
                .iter()
                .map(|parameter| {
                    parameter
                        .name
                        .strip_prefix("r#")
                        .unwrap_or(&parameter.name)
                        .to_owned()
                })
                .collect();
            let mut wire_sorted = wire_parameter_names.clone();
            let mut raw_sorted = raw_parameter_names.clone();
            wire_sorted.sort();
            raw_sorted.sort();
            if raw_sorted.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(error(
                    "lower.duplicate_parameter",
                    format!("duplicate raw parameter names for {raw_method}"),
                ));
            }
            if raw_sorted != wire_sorted {
                return Err(error(
                    "lower.parameter_drift",
                    format!("OpenAPI/raw parameter drift for {raw_method}"),
                ));
            }

            let response_projection = if matches!(
                item.response_representation,
                Some(
                    ResponseRepresentationDefinition::Text
                        | ResponseRepresentationDefinition::BinaryBuffered
                )
            ) {
                buffered_scalar_response_projection(
                    wire_operation,
                    raw_operation,
                    item.response_representation.expect("matched representation"),
                    bindings,
                )?
            } else if item.empty_response == Some(true) {
                if !empty_response_matches(wire_operation, raw_operation)? {
                    return Err(error(
                        "lower.empty_response_drift",
                        format!("empty response drift for {operation_id}"),
                    ));
                }
                ResponseProjection::Empty
            } else if item.binary_response == Some(true) {
                let response = success_response(wire_operation)?;
                let content = response
                    .get("content")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        error(
                            "lower.binary_response_drift",
                            format!("binary response drift for {operation_id}"),
                        )
                    })?;
                if content.len() != 1 {
                    return Err(error(
                        "lower.binary_response_drift",
                        format!("binary response drift for {operation_id}"),
                    ));
                }
                let schema = content
                    .values()
                    .next()
                    .and_then(|payload| payload.get("schema"))
                    .ok_or_else(|| {
                        error(
                            "lower.binary_response_drift",
                            format!("binary response drift for {operation_id}"),
                        )
                    })?;
                if schema.get("type").and_then(Value::as_str) != Some("string")
                    || schema.get("format").and_then(Value::as_str) != Some("binary")
                {
                    return Err(error(
                        "lower.binary_response_drift",
                        format!("binary response drift for {operation_id}"),
                    ));
                }
                validate_owned_byte_stream(raw_method, &raw_operation.success_type)?;
                ResponseProjection::Binary
            } else if let Some(stream) = &item.stream {
                if request.is_none() && request_raw_parameter.is_none() {
                    return Err(error(
                        "lower.stream_request",
                        format!("stream requires a request projection: {raw_method}"),
                    ));
                }
                let schema = success_schema(wire_operation, "text/event-stream")?;
                let mut wire_item = ref_name(schema).map(str::to_owned);
                if wire_item.as_deref() != Some(stream.item.as_str()) {
                    if let Some(envelope_name) = wire_item.clone() {
                        let envelope = index.schema(&envelope_name)?;
                        wire_item = envelope
                            .get("properties")
                            .and_then(|properties| properties.get("data"))
                            .and_then(ref_name)
                            .map(str::to_owned);
                        let required = envelope
                            .get("required")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .any(|field| field == "data");
                        if !required {
                            return Err(error(
                                "lower.stream_envelope",
                                format!("stream envelope has no required data: {raw_method}"),
                            ));
                        }
                    }
                }
                if wire_item.as_deref() != Some(stream.item.as_str()) {
                    return Err(error(
                        "lower.stream_drift",
                        format!("stream payload drift for {raw_method}"),
                    ));
                }
                let _ = bindings.fields(&stream.item)?;
                let wrapper = stream
                    .wrapper
                    .clone()
                    .unwrap_or_else(|| stream.item.clone());
                let wrapper_model = models
                    .iter()
                    .find(|model| model.name == wrapper)
                    .ok_or_else(|| {
                        error(
                            "lower.stream_wrapper",
                            format!("stream wrapper must own the configured item: {wrapper}"),
                        )
                    })?;
                if wrapper_model.raw != stream.item
                    || !matches!(&wrapper_model.render, ModelRenderSpec::View(view) if !view.borrowed)
                {
                    return Err(error(
                        "lower.stream_wrapper",
                        format!("stream wrapper must own the configured item: {wrapper}"),
                    ));
                }
                validate_owned_byte_stream(raw_method, &raw_operation.success_type)?;
                ResponseProjection::Sse(StreamPolicy {
                    item: stream.item.clone(),
                    wrapper,
                    type_name: stream.type_name.clone(),
                })
            } else if let Some(response) = response {
                let model = models
                    .iter()
                    .find(|model| model.name == response)
                    .expect("known model");
                if !response_matches(&index, operation_id, &model.raw, raw_operation, bindings)? {
                    return Err(error(
                        "lower.response_drift",
                        format!("OpenAPI response drift for {operation_id}"),
                    ));
                }
                if raw_operation.success_type != model.raw {
                    return Err(error(
                        "lower.raw_response_drift",
                        format!("raw response drift for {raw_method}"),
                    ));
                }
                ResponseProjection::Json {
                    model: response.into(),
                    raw: model.raw.clone(),
                }
            } else {
                return Err(error(
                    "lower.response_projection",
                    format!("operation {operation_id} needs a response projection"),
                ));
            };

            let request_projection = if let Some(request) = request {
                RequestProjection::Model {
                    media: item.request_media.unwrap_or(RequestMediaDefinition::Json),
                    model: request.into(),
                    raw: request_raw.expect("request raw"),
                    overrides: item
                        .request_overrides
                        .clone()
                        .unwrap_or_default()
                        .into_iter()
                        .collect(),
                }
            } else if let Some(media) = item.request_media.filter(|media| {
                matches!(
                    media,
                    RequestMediaDefinition::OctetStream
                        | RequestMediaDefinition::Binary
                        | RequestMediaDefinition::TextPlain
                )
            }) {
                RequestProjection::Raw {
                    media,
                    raw_parameter: request_raw_parameter.expect("raw request parameter"),
                    public_name: raw_request_public_name.expect("raw request public name"),
                }
            } else if raw_operation.parameters.is_empty() {
                RequestProjection::None
            } else {
                RequestProjection::Parameters
            };

            let mut operation = OperationSpec {
                name: public_name.clone(),
                operation_id: operation_id.clone(),
                raw_method: raw_method.clone(),
                raw_signature: RawSignature {
                    parameters: raw_operation
                        .parameters
                        .iter()
                        .map(|parameter| RawParameter {
                            name: parameter.name.clone(),
                            type_name: parameter.type_name.clone(),
                        })
                        .collect(),
                    return_type: raw_operation.return_type.clone(),
                    success_type: raw_operation.success_type.clone(),
                },
                request_projection,
                response_projection,
                call: OperationCall {
                    arguments: String::new(),
                    raw_arguments: String::new(),
                    default_raw_arguments: None,
                },
                parameter_request: None,
            };
            operation.call = operation_call(&operation, &resource, bindings)?;
            operation.parameter_request = parameter_request(&resource, &operation, bindings)?;
            resource.operations.push(operation);
        }
        resources.push(resource);
    }

    let ir = FacadeIr {
        client_name: definition.client.name.clone(),
        models,
        resources,
    };
    validate_symbols(&ir, bindings)?;
    validate_runtime(&ir, runtime)?;
    Ok(ir)
}
