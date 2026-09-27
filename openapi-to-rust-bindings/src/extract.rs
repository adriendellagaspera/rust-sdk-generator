//! Canonical Bindings v4 normalization from upstream bindings metadata plus
//! the residual invocation evidence that metadata v1 does not expose yet.

use crate::details::{
    StreamTransportEvidence, inspect_metadata_backed_details, prove_client_layout,
};
use crate::rust_type::canonical_rust_type;
use crate::semantic::RepresentationEvidence;
use crate::structural::StructuralEvidence;
use crate::upstream_metadata::UpstreamMetadata;
use crate::{Bindings, Error};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::path::Path;

fn extraction_error(code: &str, detail: impl std::fmt::Display) -> Error {
    Error::new(format!("{code}: {detail}"))
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

/// Normalize current openapi-to-rust bindings metadata into canonical Bindings v4.
///
/// Upstream metadata is authoritative for emitted symbols, signatures, source
/// identities and response planning. Generated client source is inspected only
/// for residual invocation details not represented by metadata v1.
pub fn extract_bindings(
    generated: impl AsRef<Path>,
    effective_openapi: impl AsRef<Path>,
) -> Result<Bindings, Error> {
    let upstream = UpstreamMetadata::load(&generated)?;
    let structural = upstream.structural_evidence()?;
    prove_client_layout(&generated, &structural)?;
    let semantic = upstream.semantic_evidence(&effective_openapi)?;

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

    let details =
        inspect_metadata_backed_details(&generated, &effective_openapi, &structural, &semantic)?;
    let canonical = upstream.normalize_structural()?;
    let metadata_operations = upstream.operations()?;
    let non_stream_sources = semantic
        .operations
        .values()
        .filter(|operation| {
            !matches!(
                operation.representation,
                RepresentationEvidence::EventStream { .. }
                    | RepresentationEvidence::BinaryStream { .. }
            )
        })
        .map(|operation| operation.source_operation.clone())
        .collect::<BTreeSet<_>>();

    let mut operations = Map::new();
    for (name, semantics) in &semantic.operations {
        let metadata_operation = metadata_operations
            .get(name)
            .ok_or_else(|| extraction_error("metadata.operation_missing", name))?;
        let success_type = metadata_operation.success_type()?;
        let parameters = metadata_operation
            .signature
            .arguments
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
        let streaming = matches!(
            semantics.representation,
            RepresentationEvidence::EventStream { .. }
                | RepresentationEvidence::BinaryStream { .. }
        );
        if streaming
            && detail.stream.is_none()
            && non_stream_sources.contains(&semantics.source_operation)
        {
            continue;
        }

        let kind = if metadata_operation.operation.multipart_filenames {
            "multipart_filenames"
        } else {
            "call_shape"
        };
        let (stream, stream_transport) = if streaming {
            let evidence = detail
                .stream
                .as_ref()
                .ok_or_else(|| extraction_error("extract.stream_abi_unproven", name))?;
            let transport = match &evidence.transport {
                StreamTransportEvidence::NamedAlias {
                    alias,
                    native_type,
                    wasm_type,
                } => json!({
                    "kind": "named_alias",
                    "alias": alias,
                    "native_type": native_type,
                    "wasm_type": wasm_type,
                }),
                StreamTransportEvidence::AnonymousImplTrait { rust_type } => json!({
                    "kind": "anonymous_impl_trait",
                    "rust_type": rust_type,
                }),
            };
            (
                json!({
                    "item_type": evidence.item_type,
                    "error_type": evidence.error_type,
                    "lifetime": evidence.lifetime,
                }),
                transport,
            )
        } else {
            (Value::Null, Value::Null)
        };

        let parameter_wires = detail
            .parameter_wires
            .iter()
            .map(|wire| {
                json!({
                    "rust_name": wire.rust_name,
                    "location": wire.location,
                    "wire_name": wire.wire_name,
                })
            })
            .collect::<Vec<_>>();
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
        let mut metadata = json!({
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
            "stream_transport": stream_transport,
        });
        if !parameter_wires.is_empty() {
            metadata["parameter_wires"] = json!(parameter_wires);
        }
        if detail.request_discriminator_unproven {
            metadata["request_discriminator_unproven"] = Value::Bool(true);
        }
        operations.insert(
            name.clone(),
            json!({
                "name": name,
                "parameters": parameters,
                "return_type": canonical_rust_type(&metadata_operation.signature.return_type)?,
                "success_type": success_type,
                "stream": stream,
                "metadata": metadata,
            }),
        );
    }

    Bindings::from_value(json!({
        "schema_version": 4,
        "structs": canonical.structs,
        "enums": canonical.enums,
        "aliases": canonical.aliases,
        "operations": operations,
        "symbol_paths": canonical.symbols,
        "binding": client_layout(&structural)?,
    }))
    .map_err(|error| extraction_error("extract.canonical_bindings_invalid", error))
}
