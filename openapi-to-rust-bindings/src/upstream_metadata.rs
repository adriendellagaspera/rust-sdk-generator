//! Primary normalization of openapi-to-rust v0.19+ binding metadata.
//!
//! The producer collects this artifact from the same AST and client method plans
//! used to render Rust. Generated-source inspection remains a complementary
//! proof for details metadata v1 does not expose yet (wire parameter mapping,
//! request discriminator assignments and stream ownership).
use crate::rust_type::canonical_rust_type;
use crate::semantic::{
    OperationSemanticEvidence, RepresentationEvidence, SemanticEvidence, SourceOperationEvidence,
};
use crate::structural::EvidenceLocation;
use crate::{Error, structural::StructuralEvidence};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use syn::{GenericArgument, PathArguments, Type};

pub(crate) const METADATA_NAME: &str = "bindings.json";
const METADATA_SCHEMA_VERSION: u32 = 1;

fn failure(code: &str, detail: impl std::fmt::Display) -> Error {
    Error::new(format!("{code}: {detail}"))
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpstreamMetadata {
    schema_version: u32,
    generator_version: String,
    module_label: String,
    coverage: Vec<String>,
    modules: Vec<String>,
    reexports: Vec<String>,
    symbols: Vec<BindingSymbol>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingSymbol {
    path: String,
    name: String,
    kind: String,
    attributes: Vec<String>,
    generics: String,
    impl_generics: Option<String>,
    impl_trait: Option<String>,
    rust_type: Option<String>,
    fields: Vec<BindingField>,
    variants: Vec<BindingVariant>,
    signature: Option<BindingSignature>,
    operation: Option<BindingOperation>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingField {
    name: Option<String>,
    index: usize,
    public: bool,
    wire_name: Option<String>,
    rust_type: String,
    attributes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingVariant {
    name: String,
    wire_name: Option<String>,
    attributes: Vec<String>,
    fields: Vec<BindingField>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BindingSignature {
    rust: String,
    asynchronous: bool,
    receiver: Option<String>,
    pub(crate) arguments: Vec<BindingArgument>,
    pub(crate) return_type: String,
    generics: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BindingArgument {
    pub(crate) name: String,
    pub(crate) rust_type: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BindingOperation {
    pub(crate) operation_id: String,
    pub(crate) source_json_pointer: String,
    pub(crate) source_method: String,
    pub(crate) source_path: String,
    pub(crate) source_operation_id: Option<String>,
    pub(crate) webhook: bool,
    pub(crate) response_statuses: Vec<String>,
    pub(crate) response_excluded_statuses: Vec<String>,
    pub(crate) response_media_type: Option<String>,
    pub(crate) response_kind: String,
    pub(crate) consumption: String,
    pub(crate) multipart_filenames: bool,
}

pub(crate) struct CanonicalMetadata {
    pub structs: Map<String, Value>,
    pub enums: Map<String, Value>,
    pub aliases: Map<String, Value>,
    pub symbols: Map<String, Value>,
}

pub(crate) struct MetadataOperation<'a> {
    pub signature: &'a BindingSignature,
    pub operation: &'a BindingOperation,
}

impl UpstreamMetadata {
    pub(crate) fn load(generated: impl AsRef<Path>) -> Result<Self, Error> {
        let path = generated.as_ref().join(METADATA_NAME);
        let source = fs::read_to_string(&path).map_err(|error| {
            failure(
                "metadata.unreadable",
                format!("{}: {error}", path.display()),
            )
        })?;
        let metadata: Self = serde_json::from_str(&source)
            .map_err(|error| failure("metadata.invalid_json", error))?;
        metadata.validate()?;
        Ok(metadata)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.schema_version != METADATA_SCHEMA_VERSION {
            return Err(failure(
                "metadata.schema_version",
                format!(
                    "unsupported openapi-to-rust bindings metadata version {}",
                    self.schema_version
                ),
            ));
        }
        if self.generator_version.trim().is_empty() || self.module_label.trim().is_empty() {
            return Err(failure(
                "metadata.identity",
                "generator_version and module_label must be non-empty",
            ));
        }
        for required in ["models", "public_helpers", "http_client"] {
            if !self.coverage.iter().any(|value| value == required) {
                return Err(failure(
                    "metadata.coverage",
                    format!("required coverage {required:?} is missing"),
                ));
            }
        }
        if self.modules.is_empty() || self.reexports.iter().any(|value| value.is_empty()) {
            return Err(failure(
                "metadata.layout",
                "metadata modules/reexports are malformed",
            ));
        }
        let mut identities = BTreeSet::new();
        for symbol in &self.symbols {
            if symbol.path.is_empty() || symbol.name.is_empty() || symbol.kind.is_empty() {
                return Err(failure("metadata.symbol", "empty symbol identity"));
            }
            let identity = (
                symbol.path.as_str(),
                symbol.kind.as_str(),
                symbol.impl_trait.as_deref(),
                symbol.attributes.as_slice(),
            );
            if !identities.insert(identity) {
                return Err(failure(
                    "metadata.symbol_collision",
                    format!("duplicate symbol {}", symbol.path),
                ));
            }
            if let Some(signature) = &symbol.signature {
                if signature.rust.is_empty() || signature.return_type.is_empty() {
                    return Err(failure(
                        "metadata.signature",
                        format!("{} has an incomplete signature", symbol.path),
                    ));
                }
                let _ = (&signature.generics, signature.asynchronous, &signature.receiver);
            }
            let _ = (&symbol.generics, &symbol.impl_generics);
        }
        Ok(())
    }

    pub(crate) fn operations(&self) -> Result<BTreeMap<String, MetadataOperation<'_>>, Error> {
        let mut output = BTreeMap::new();
        for symbol in &self.symbols {
            let Some(operation) = symbol.operation.as_ref() else {
                continue;
            };
            if symbol.kind != "method" {
                return Err(failure(
                    "metadata.operation_symbol",
                    format!("{} is not a method", symbol.path),
                ));
            }
            let signature = symbol.signature.as_ref().ok_or_else(|| {
                failure(
                    "metadata.signature",
                    format!("{} has operation metadata but no signature", symbol.path),
                )
            })?;
            if output
                .insert(
                    symbol.name.clone(),
                    MetadataOperation {
                        signature,
                        operation,
                    },
                )
                .is_some()
            {
                return Err(failure(
                    "metadata.operation_collision",
                    format!("duplicate generated method {}", symbol.name),
                ));
            }
        }
        Ok(output)
    }

    pub(crate) fn normalize_structural(&self) -> Result<CanonicalMetadata, Error> {
        let operations = self.operations()?;
        let referenced_client_enums = operations
            .values()
            .flat_map(|operation| operation.signature.arguments.iter())
            .flat_map(|argument| words(&argument.rust_type))
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>();

        let mut structs = Map::new();
        let mut enums = Map::new();
        let mut aliases = Map::new();
        let mut symbols = Map::new();

        for symbol in &self.symbols {
            let is_model = symbol.path.starts_with("types::");
            let is_client_enum =
                symbol.path.starts_with("client::")
                    && symbol.kind == "enum"
                    && referenced_client_enums.contains(&symbol.name);
            if !is_model && !is_client_enum {
                continue;
            }
            if !matches!(symbol.kind.as_str(), "struct" | "enum" | "alias") {
                continue;
            }
            if structs.contains_key(&symbol.name)
                || enums.contains_key(&symbol.name)
                || aliases.contains_key(&symbol.name)
            {
                return Err(failure("metadata.symbol_collision", &symbol.name));
            }

            match symbol.kind.as_str() {
                "struct" => {
                    if symbol.fields.iter().any(|field| !field.public) {
                        let base = symbol.name.strip_suffix("Builder");
                        if base.is_some_and(|base| {
                            self.symbols.iter().any(|candidate| {
                                candidate.path == format!("types::{base}")
                                    && matches!(candidate.kind.as_str(), "struct" | "enum" | "alias")
                            })
                        }) {
                            continue;
                        }
                        return Err(failure(
                            "metadata.unhandled_model_shape",
                            format!("{} contains private fields", symbol.path),
                        ));
                    }
                    let fields = symbol
                        .fields
                        .iter()
                        .map(|field| {
                            let name = field.name.clone().ok_or_else(|| {
                                failure(
                                    "metadata.unhandled_model_shape",
                                    format!("{} has an unnamed struct field", symbol.path),
                                )
                            })?;
                            let _ = (field.index, &field.attributes);
                            Ok(json!({
                                "name": name,
                                "wire_name": field.wire_name,
                                "type": canonical_rust_type(&field.rust_type)?,
                            }))
                        })
                        .collect::<Result<Vec<_>, Error>>()?;
                    structs.insert(symbol.name.clone(), Value::Array(fields));
                }
                "enum" => {
                    let variants = symbol
                        .variants
                        .iter()
                        .map(|variant| {
                            let _ = &variant.attributes;
                            let payload = match variant.fields.as_slice() {
                                [] => Value::Null,
                                [field] if field.name.is_none() => {
                                    Value::String(canonical_rust_type(&field.rust_type)?)
                                }
                                [..] => {
                                    return Err(failure(
                                        "metadata.enum_encoding_unproven",
                                        format!(
                                            "{}::{} has named or multiple payload fields",
                                            symbol.path, variant.name
                                        ),
                                    ));
                                }
                            };
                            Ok(json!({
                                "name": variant.name,
                                "payload": payload,
                                "wire_name": if variant.fields.is_empty() {
                                    variant.wire_name.clone().map(Value::String).unwrap_or(Value::Null)
                                } else {
                                    Value::Null
                                },
                            }))
                        })
                        .collect::<Result<Vec<_>, Error>>()?;
                    enums.insert(symbol.name.clone(), Value::Array(variants));
                }
                "alias" => {
                    let spelling = symbol.rust_type.as_deref().ok_or_else(|| {
                        failure(
                            "metadata.alias_target_unproven",
                            format!("{} has no alias target", symbol.path),
                        )
                    })?;
                    if !symbol.attributes.is_empty() {
                        return Err(failure(
                            "metadata.alias_target_unproven",
                            format!("{} is target/attribute conditional", symbol.path),
                        ));
                    }
                    aliases.insert(
                        symbol.name.clone(),
                        Value::String(canonical_rust_type(spelling)?),
                    );
                }
                _ => unreachable!(),
            }
            let qualified = format!("crate::generated::{}", symbol.path);
            if symbols
                .insert(symbol.name.clone(), Value::String(qualified.clone()))
                .is_some()
            {
                return Err(failure(
                    "metadata.symbol_collision",
                    format!("{} conflicts at {qualified}", symbol.name),
                ));
            }
        }
        Ok(CanonicalMetadata {
            structs,
            enums,
            aliases,
            symbols,
        })
    }

    pub(crate) fn semantic_evidence(
        &self,
        effective_openapi: impl AsRef<Path>,
    ) -> Result<SemanticEvidence, Error> {
        let openapi: Value = serde_json::from_str(
            &fs::read_to_string(effective_openapi.as_ref()).map_err(|error| {
                failure(
                    "metadata.openapi_unreadable",
                    format!("{}: {error}", effective_openapi.as_ref().display()),
                )
            })?,
        )
        .map_err(|error| failure("metadata.openapi_invalid_json", error))?;
        let operations = self.operations()?;
        let mut output = BTreeMap::new();
        let mut represented_sources = BTreeSet::new();
        let mut unsupported_stream_methods = Vec::new();

        for (name, evidence) in operations {
            let operation = evidence.operation;
            if operation.webhook {
                return Err(failure(
                    "metadata.webhook_unsupported",
                    format!("{name}: webhook operations are outside the current Bindings envelope"),
                ));
            }
            if !operation.response_excluded_statuses.is_empty() {
                return Err(failure(
                    "metadata.status_exclusions_unsupported",
                    format!(
                        "{name}: status exclusions {:?} are not representable in Bindings v3",
                        operation.response_excluded_statuses
                    ),
                ));
            }
            let source = openapi.pointer(&operation.source_json_pointer).ok_or_else(|| {
                failure(
                    "metadata.source_pointer",
                    format!(
                        "{name}: source pointer {} is absent from the effective OpenAPI",
                        operation.source_json_pointer
                    ),
                )
            })?;
            let source_operation_id = operation
                .source_operation_id
                .as_deref()
                .unwrap_or(&operation.operation_id);
            if source
                .get("operationId")
                .and_then(Value::as_str)
                .is_some_and(|value| value != source_operation_id)
            {
                return Err(failure(
                    "metadata.source_identity",
                    format!("{name}: source operationId disagrees with metadata"),
                ));
            }
            let success_type = success_type(&evidence.signature.return_type)?;
            let media_type = operation.response_media_type.clone();
            let representation = match operation.response_kind.as_str() {
                "json" => RepresentationEvidence::Json {
                    schema_name: success_type,
                    media_type: media_type.ok_or_else(|| {
                        failure("metadata.response_media", format!("{name}: JSON media is missing"))
                    })?,
                },
                "text" => RepresentationEvidence::Text {
                    media_type: media_type.ok_or_else(|| {
                        failure("metadata.response_media", format!("{name}: text media is missing"))
                    })?,
                },
                "binary" if operation.consumption == "binary_stream" => {
                    let media_type = media_type.ok_or_else(|| {
                        failure("metadata.response_media", format!("{name}: binary media is missing"))
                    })?;
                    RepresentationEvidence::BinaryStream {
                        wildcard: media_type.contains('*'),
                        media_type,
                    }
                }
                "binary" => {
                    let media_type = media_type.ok_or_else(|| {
                        failure("metadata.response_media", format!("{name}: binary media is missing"))
                    })?;
                    RepresentationEvidence::BinaryBuffered {
                        wildcard: media_type.contains('*'),
                        media_type,
                    }
                }
                "event_stream" => RepresentationEvidence::EventStream {
                    media_type: media_type.ok_or_else(|| {
                        failure("metadata.response_media", format!("{name}: SSE media is missing"))
                    })?,
                },
                "empty" => RepresentationEvidence::Empty,
                other => {
                    return Err(failure(
                        "metadata.response_kind",
                        format!("{name}: unsupported response kind {other:?}"),
                    ));
                }
            };
            if matches!(
                representation,
                RepresentationEvidence::EventStream { .. }
                    | RepresentationEvidence::BinaryStream { .. }
            ) {
                unsupported_stream_methods.push(name.clone());
            }
            let source_operation = SourceOperationEvidence {
                operation_id: source_operation_id.to_owned(),
                method: operation.source_method.to_ascii_uppercase(),
                path: operation.source_path.clone(),
            };
            represented_sources.insert(source_operation.clone());
            output.insert(
                name.clone(),
                OperationSemanticEvidence {
                    rust_method_name: name,
                    source_operation,
                    emitted_operation_id: operation.operation_id.clone(),
                    representation,
                    success_statuses: operation.response_statuses.clone(),
                    location: EvidenceLocation {
                        file: METADATA_NAME.into(),
                        line: 1,
                        module: "client".into(),
                    },
                },
            );
            let _ = operation.multipart_filenames;
        }

        let mut unmatched_source_operations = Vec::new();
        let Some(paths) = openapi.get("paths").and_then(Value::as_object) else {
            return Err(failure("metadata.openapi_paths", "effective OpenAPI has no paths object"));
        };
        for (path, item) in paths {
            let Some(item) = item.as_object() else { continue };
            for (method, operation) in item {
                if !matches!(
                    method.as_str(),
                    "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "trace"
                ) {
                    continue;
                }
                let Some(operation_id) = operation.get("operationId").and_then(Value::as_str) else {
                    return Err(failure(
                        "metadata.source_operation_id",
                        format!("{} {} has no operationId", method.to_ascii_uppercase(), path),
                    ));
                };
                let identity = SourceOperationEvidence {
                    operation_id: operation_id.to_owned(),
                    method: method.to_ascii_uppercase(),
                    path: path.clone(),
                };
                if !represented_sources.contains(&identity) {
                    unmatched_source_operations.push(identity);
                }
            }
        }

        Ok(SemanticEvidence {
            operations: output,
            unsupported_stream_methods,
            unmatched_source_operations,
        })
    }

    pub(crate) fn verify_client_signatures(
        &self,
        structural: &StructuralEvidence,
    ) -> Result<(), Error> {
        let structural_methods = structural
            .client
            .methods
            .iter()
            .map(|method| (method.name.as_str(), method))
            .collect::<BTreeMap<_, _>>();
        for (name, operation) in self.operations()? {
            let method = structural_methods.get(name.as_str()).ok_or_else(|| {
                failure(
                    "metadata.signature_drift",
                    format!("{name}: generated Rust method is missing"),
                )
            })?;
            let metadata_return = canonical_rust_type(&operation.signature.return_type)?;
            let structural_return = canonical_rust_type(&method.return_type)?;
            if metadata_return != structural_return {
                return Err(failure(
                    "metadata.signature_drift",
                    format!(
                        "{name}: metadata return {metadata_return} != generated Rust {structural_return}"
                    ),
                ));
            }
            if operation.signature.arguments.len() != method.parameters.len() {
                return Err(failure(
                    "metadata.signature_drift",
                    format!("{name}: parameter count disagrees with generated Rust"),
                ));
            }
            for (metadata, actual) in operation
                .signature
                .arguments
                .iter()
                .zip(&method.parameters)
            {
                if metadata.name != actual.name
                    || canonical_rust_type(&metadata.rust_type)?
                        != canonical_rust_type(&actual.rust_type)?
                {
                    return Err(failure(
                        "metadata.signature_drift",
                        format!("{name}: parameter metadata disagrees with generated Rust"),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn success_type(return_type: &str) -> Result<String, Error> {
    let parsed: Type = syn::parse_str(return_type)
        .map_err(|error| failure("metadata.return_type", format!("{return_type:?}: {error}")))?;
    let Type::Path(path) = parsed else {
        return Err(failure(
            "metadata.return_type",
            format!("expected Result return type, got {return_type}"),
        ));
    };
    let segment = path.path.segments.last().ok_or_else(|| {
        failure("metadata.return_type", format!("empty return path {return_type}"))
    })?;
    if segment.ident != "Result" {
        return Err(failure(
            "metadata.return_type",
            format!("expected Result return type, got {return_type}"),
        ));
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(failure("metadata.return_type", return_type));
    };
    let Some(GenericArgument::Type(success)) = arguments.args.first() else {
        return Err(failure("metadata.return_type", return_type));
    };
    canonical_rust_type(&success.to_token_stream().to_string())
}

fn words(value: &str) -> BTreeSet<&str> {
    value
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|word| !word.is_empty())
        .collect()
}

use quote::ToTokens;
