//! Canonical Bindings v3 normalization from proved structural + semantic
//! evidence. This remains separate from read_bindings until #151 cuts over the
//! default manifest-free path.
use crate::details::{OperationKindEvidence, inspect_details};
use crate::rust_type::canonical_rust_type;
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
            names.extend(
                words(&parameter.rust_type)
                    .into_iter()
                    .map(ToOwned::to_owned),
            );
        }
    }
    names
}

fn insert_symbol(symbols: &mut Map<String, Value>, name: &str, path: &str) -> Result<(), Error> {
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
            "payload": if let Value::String(value) = payload {
                Value::String(canonical_rust_type(&value)?)
            } else {
                payload
            },
            "wire_name": wire_name,
        }));
    }
    Ok(Value::Array(variants))
}

struct CanonicalStructural {
    structs: Map<String, Value>,
    enums: Map<String, Value>,
    aliases: Map<String, Value>,
    symbols: Map<String, Value>,
}

fn normalize_structural(structural: &StructuralEvidence) -> Result<CanonicalStructural, Error> {
    let mut structs = Map::new();
    let mut enums = Map::new();
    let mut aliases = Map::new();
    let mut symbols = Map::new();

    for (path, item) in &structural.structs {
        if !model_symbol(path) {
            continue;
        }
        let name = short_symbol(path)?;
        if item.has_private_fields {
            let is_builder = name
                .strip_suffix("Builder")
                .is_some_and(|model| structural.structs.contains_key(&format!(
                    "crate::generated::types::{model}"
                )));
            if is_builder {
                continue;
            }
            return Err(extraction_error(
                "extract.unhandled_model_shape",
                format!("{path} contains private fields"),
            ));
        }
        if structs.contains_key(name) || enums.contains_key(name) || aliases.contains_key(name) {
            return Err(extraction_error("extract.symbol_collision", name));
        }
        let fields = item
            .fields
            .iter()
            .map(|field| {
                Ok(json!({
                    "name": field.name,
                    "wire_name": field.wire_name,
                    "type": canonical_rust_type(&field.rust_type)?,
                }))
            })
            .collect::<Result<Vec<_>, Error>>()?;
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
        aliases.insert(
            name.to_owned(),
            Value::String(canonical_rust_type(&definitions[0].rust_type)?),
        );
        insert_symbol(&mut symbols, name, path)?;
    }

    Ok(CanonicalStructural {
        structs,
        enums,
        aliases,
        symbols,
    })
}

fn representation_json(value: &RepresentationEvidence) -> Result<Value, Error> {
    serde_json::to_value(value)
        .map_err(|error| extraction_error("extract.representation_serialization_failed", error))
}

fn client_layout(structural: &StructuralEvidence) -> Result<Value, Error> {
    if !structural
        .client
        .constructors
        .iter()
        .any(|name| name == "new")
    {
        return Err(extraction_error(
            "extract.client_layout_unproven",
            "openapi-to-rust client has no public new constructor",
        ));
    }
    for required in ["with_api_key", "with_base_url"] {
        if !structural
            .client
            .builders
            .iter()
            .any(|name| name == required)
        {
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
    let semantic = inspect_semantics(&generated, &effective_openapi)?;

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
    let details = inspect_details(
        &generated,
        &effective_openapi,
        &structural,
        &semantic,
    )?;

    let canonical = normalize_structural(&structural)?;
    let signatures: BTreeMap<_, _> = structural
        .client
        .methods
        .iter()
        .map(|method| (method.name.as_str(), method))
        .collect();

    let mut operations = Map::new();
    for (name, semantics) in &semantic.operations {
        let signature = signatures
            .get(name.as_str())
            .ok_or_else(|| extraction_error("extract.signature_missing", name))?;
        let success_type = signature
            .success_type
            .as_ref()
            .ok_or_else(|| extraction_error("extract.success_type_unproven", name))?;
        let parameters = signature
            .parameters
            .iter()
            .map(|parameter| {
                Ok(json!({
                    "name": parameter.name,
                    "type": canonical_rust_type(&parameter.rust_type)?,
                }))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let detail = details
            .get(name)
            .ok_or_else(|| extraction_error("extract.operation_details_missing", name))?;
        let kind = match detail.kind {
            OperationKindEvidence::CallShape => "call_shape",
            OperationKindEvidence::MultipartFilenames => "multipart_filenames",
        };
        let (stream, stream_abi) = if matches!(
            semantics.representation,
            RepresentationEvidence::EventStream { .. }
                | RepresentationEvidence::BinaryStream { .. }
        ) {
            let abi = detail.stream.as_ref().ok_or_else(|| {
                extraction_error("extract.stream_abi_unproven", name)
            })?;
            (
                json!({
                    "item_type": abi.item_type,
                    "error_type": abi.error_type,
                    "lifetime": abi.lifetime,
                }),
                json!({
                    "alias": abi.alias,
                    "item_type": abi.item_type,
                    "error_type": abi.error_type,
                    "lifetime": abi.lifetime,
                    "native_type": abi.native_type,
                    "wasm_type": abi.wasm_type,
                }),
            )
        } else {
            (Value::Null, Value::Null)
        };
        let request_discriminators = detail
            .request_discriminators
            .iter()
            .map(|discriminator| {
                json!({
                    "wire_name": discriminator.wire_name,
                    "rust_access_path": discriminator.rust_access_path,
                    "rust_value_type": discriminator.rust_value_type,
                    "value": discriminator.value,
                    "field_required": discriminator.field_required,
                    "field_nullable": discriminator.field_nullable,
                    "field_tri_state": discriminator.field_tri_state,
                })
            })
            .collect::<Vec<_>>();
        operations.insert(
            name.clone(),
            json!({
                "name": name,
                "parameters": parameters,
                "return_type": canonical_rust_type(&signature.return_type)?,
                "success_type": canonical_rust_type(success_type)?,
                "stream": stream,
                "metadata": {
                    "kind": kind,
                    "source_operation": {
                        "operation_id": semantics.source_operation.operation_id,
                        "method": semantics.source_operation.method,
                        "path": semantics.source_operation.path,
                    },
                    "emitted_operation_id": semantics.emitted_operation_id,
                    "representation": representation_json(&semantics.representation)?,
                    "success_statuses": semantics.success_statuses,
                    "request_discriminators": request_discriminators,
                    "stream_abi": stream_abi,
                }
            }),
        );
    }

    Bindings::from_value(json!({
        "schema_version": 3,
        "structs": canonical.structs,
        "enums": canonical.enums,
        "aliases": canonical.aliases,
        "operations": operations,
        "symbol_paths": canonical.symbols,
        "binding": client_layout(&structural)?,
    }))
    .map_err(|error| extraction_error("extract.canonical_bindings_invalid", error))
}
