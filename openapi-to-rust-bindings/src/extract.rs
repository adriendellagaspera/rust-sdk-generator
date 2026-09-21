//! Canonical Bindings v3 normalization from proved structural + semantic
//! evidence. This remains separate from read_bindings until #151 cuts over the
//! default manifest-free path.
use crate::semantic::{RepresentationEvidence, inspect_semantics};
use crate::structural::{EnumEvidence, StructuralEvidence, inspect_generated};
use crate::{Bindings, Error};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn extraction_error(code: &str, detail: impl std::fmt::Display) -> Error {
    Error::new(format!("{code}: {detail}"))
}

fn short_symbol(path: &str) -> Result<&str, Error> {
    path.rsplit("::")
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| extraction_error("extract.symbol_unresolved", path))
}

fn model_symbol(path: &str) -> bool {
    path.starts_with("crate::generated::types::")
}

fn words(value: &str) -> BTreeSet<&str> {
    value
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|word| !word.is_empty())
        .collect()
}

fn referenced_client_enum_names(structural: &StructuralEvidence) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for method in &structural.client.methods {
        for parameter in &method.parameters {
            names.extend(words(&parameter.rust_type).into_iter().map(ToOwned::to_owned));
        }
    }
    names
}

fn insert_symbol(
    symbols: &mut Map<String, Value>,
    name: &str,
    path: &str,
) -> Result<(), Error> {
    if let Some(previous) = symbols.insert(name.to_owned(), Value::String(path.to_owned())) {
        return Err(extraction_error(
            "extract.symbol_collision",
            format!("{name}: {previous} conflicts with {path}"),
        ));
    }
    Ok(())
}

fn enum_json(item: &EnumEvidence, name: &str) -> Result<Value, Error> {
    let mut variants = Vec::new();
    for variant in &item.variants {
        if !variant.named_payload.is_empty() {
            return Err(extraction_error(
                "extract.enum_encoding_unproven",
                format!("{name}::{} has named payload fields", variant.name),
            ));
        }
        let payload = match variant.payload.as_slice() {
            [] => Value::Null,
            [payload] => Value::String(payload.clone()),
            _ => {
                return Err(extraction_error(
                    "extract.enum_encoding_unproven",
                    format!("{name}::{} has multiple positional payloads", variant.name),
                ));
            }
        };
        let wire_name = if payload.is_null() {
            Value::String(variant.wire_name.clone())
        } else {
            Value::Null
        };
        variants.push(json!({
            "name": variant.name,
            "payload": payload,
            "wire_name": wire_name,
        }));
    }
    Ok(Value::Array(variants))
}

fn normalize_structural(
    structural: &StructuralEvidence,
) -> Result<(Map<String, Value>, Map<String, Value>, Map<String, Value>, Map<String, Value>), Error>
{
    let mut structs = Map::new();
    let mut enums = Map::new();
    let mut aliases = Map::new();
    let mut symbols = Map::new();

    for (path, item) in &structural.structs {
        if !model_symbol(path) {
            continue;
        }
        if item.has_private_fields {
            return Err(extraction_error(
                "extract.unhandled_model_shape",
                format!("{path} contains private fields"),
            ));
        }
        let name = short_symbol(path)?;
        if structs.contains_key(name) || enums.contains_key(name) || aliases.contains_key(name) {
            return Err(extraction_error("extract.symbol_collision", name));
        }
        let fields = item
            .fields
            .iter()
            .map(|field| {
                json!({
                    "name": field.name,
                    "wire_name": field.wire_name,
                    "type": field.rust_type,
                })
            })
            .collect();
        structs.insert(name.to_owned(), Value::Array(fields));
        insert_symbol(&mut symbols, name, path)?;
    }

    let referenced_client_enums = referenced_client_enum_names(structural);
    for (path, item) in &structural.enums {
        let name = short_symbol(path)?;
        let include = model_symbol(path)
            || (path.starts_with("crate::generated::client::")
                && referenced_client_enums.contains(name));
        if !include {
            continue;
        }
        if structs.contains_key(name) || enums.contains_key(name) || aliases.contains_key(name) {
            return Err(extraction_error("extract.symbol_collision", name));
        }
        enums.insert(name.to_owned(), enum_json(item, name)?);
        insert_symbol(&mut symbols, name, path)?;
    }

    for (path, definitions) in &structural.aliases {
        if !model_symbol(path) {
            continue;
        }
        let name = short_symbol(path)?;
        if definitions.len() != 1 || !definitions[0].cfg.is_empty() {
            return Err(extraction_error(
                "extract.alias_target_unproven",
                format!("{path} has target-conditional or duplicate definitions"),
            ));
        }
        if structs.contains_key(name) || enums.contains_key(name) || aliases.contains_key(name) {
            return Err(extraction_error("extract.symbol_collision", name));
        }
        aliases.insert(name.to_owned(), Value::String(definitions[0].rust_type.clone()));
        insert_symbol(&mut symbols, name, path)?;
    }

    Ok((structs, enums, aliases, symbols))
}

fn representation_json(value: &RepresentationEvidence) -> Result<Value, Error> {
    serde_json::to_value(value).map_err(|error| {
        extraction_error(
            "extract.representation_serialization_failed",
            error,
        )
    })
}

fn client_layout(structural: &StructuralEvidence) -> Result<Value, Error> {
    if !structural.client.constructors.iter().any(|name| name == "new") {
        return Err(extraction_error(
            "extract.client_layout_unproven",
            "openapi-to-rust client has no public new constructor",
        ));
    }
    for required in ["with_api_key", "with_base_url"] {
        if !structural.client.builders.iter().any(|name| name == required) {
            return Err(extraction_error(
                "extract.client_layout_unproven",
                format!("openapi-to-rust client has no {required} builder"),
            ));
        }
    }
    if !structural
        .client
        .imports
        .iter()
        .map(|value| value.replace(' ', ""))
        .any(|value| value == "super::types::*")
    {
        return Err(extraction_error(
            "extract.client_layout_unproven",
            "client does not import the generated types prelude",
        ));
    }
    Ok(json!({
        "client": {
            "type_path": structural.client.path,
            "constructor": "new",
            "api_key_builder": "with_api_key",
            "base_url_builder": "with_base_url",
        },
        "type_preludes": ["crate::generated::types::*"],
    }))
}

/// Produce canonical Bindings v3 for call shapes whose complete required
/// semantics are observable. Stream call shapes remain fail-closed until their
/// native/WASM ABI is established from emitted aliases (#150 follow-up).
pub fn extract_bindings(
    generated: impl AsRef<Path>,
    effective_openapi: impl AsRef<Path>,
) -> Result<Bindings, Error> {
    let structural = inspect_generated(&generated)?;
    let semantic = inspect_semantics(&generated, effective_openapi)?;

    if !semantic.unmatched_source_operations.is_empty() {
        return Err(extraction_error(
            "extract.source_operation_unemitted",
            semantic
                .unmatched_source_operations
                .iter()
                .map(|operation| format!("{} {}", operation.method, operation.path))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    if !semantic.unsupported_stream_methods.is_empty() {
        return Err(extraction_error(
            "extract.stream_abi_unproven",
            semantic.unsupported_stream_methods.join(", "),
        ));
    }

    let (structs, enums, aliases, symbols) = normalize_structural(&structural)?;
    let signatures: BTreeMap<_, _> = structural
        .client
        .methods
        .iter()
        .map(|method| (method.name.as_str(), method))
        .collect();

    let mut operations = Map::new();
    for (name, semantics) in &semantic.operations {
        let signature = signatures.get(name.as_str()).ok_or_else(|| {
            extraction_error(
                "extract.signature_missing",
                name,
            )
        })?;
        let success_type = signature.success_type.as_ref().ok_or_else(|| {
            extraction_error(
                "extract.success_type_unproven",
                name,
            )
        })?;
        let parameters = signature
            .parameters
            .iter()
            .map(|parameter| {
                json!({
                    "name": parameter.name,
                    "type": parameter.rust_type,
                })
            })
            .collect::<Vec<_>>();
        operations.insert(
            name.clone(),
            json!({
                "name": name,
                "parameters": parameters,
                "return_type": signature.return_type,
                "success_type": success_type,
                "stream": null,
                "metadata": {
                    "kind": "call_shape",
                    "source_operation": {
                        "operation_id": semantics.source_operation.operation_id,
                        "method": semantics.source_operation.method,
                        "path": semantics.source_operation.path,
                    },
                    "emitted_operation_id": semantics.emitted_operation_id,
                    "representation": representation_json(&semantics.representation)?,
                    "success_statuses": semantics.success_statuses,
                    "request_discriminators": [],
                    "stream_abi": null,
                }
            }),
        );
    }

    Bindings::from_value(json!({
        "schema_version": 3,
        "structs": structs,
        "enums": enums,
        "aliases": aliases,
        "operations": operations,
        "symbol_paths": symbols,
        "binding": client_layout(&structural)?,
    }))
    .map_err(|error| extraction_error("extract.canonical_bindings_invalid", error))
}
