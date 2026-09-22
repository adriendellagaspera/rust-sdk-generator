use crate::{Bindings, Error};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Generator-owned metadata emitted by openapi-to-rust.
pub const MANIFEST_NAME: &str = "binding-manifest.json";
const MANIFEST_SCHEMA: &str = "openapi-to-rust.binding-manifest";
const MANIFEST_SCHEMA_VERSION: u32 = 1;

fn deserialize_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: String,
    schema_version: u32,
    generator: ManifestGenerator,
    structs: BTreeMap<String, Vec<ManifestField>>,
    enums: BTreeMap<String, Vec<ManifestVariant>>,
    aliases: BTreeMap<String, String>,
    symbol_paths: BTreeMap<String, String>,
    operations: Vec<ManifestOperation>,
    raw_client: ManifestRawClient,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestGenerator {
    name: String,
    version: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestField {
    name: String,
    #[serde(deserialize_with = "deserialize_nullable")]
    wire_name: Option<String>,
    #[serde(rename = "type")]
    type_name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestVariant {
    name: String,
    #[serde(deserialize_with = "deserialize_nullable")]
    payload: Option<String>,
    #[serde(deserialize_with = "deserialize_nullable")]
    wire_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestParameter {
    name: String,
    #[serde(rename = "type")]
    type_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ManifestOperationKind {
    CallShape,
    MultipartFilenames,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestSourceOperation {
    operation_id: String,
    method: String,
    path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ManifestRepresentation {
    Json {
        schema_name: String,
        media_type: String,
    },
    Text {
        media_type: String,
    },
    BinaryBuffered {
        media_type: String,
        wildcard: bool,
    },
    EventStream {
        media_type: String,
    },
    BinaryStream {
        media_type: String,
        wildcard: bool,
    },
    Empty,
}

impl ManifestRepresentation {
    fn is_streaming(&self) -> bool {
        matches!(self, Self::EventStream { .. } | Self::BinaryStream { .. })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum ManifestDiscriminatorValue {
    Bool(bool),
    Integer(i64),
    String(String),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestRequestDiscriminator {
    wire_name: String,
    rust_access_path: Vec<String>,
    rust_value_type: String,
    value: ManifestDiscriminatorValue,
    field_required: bool,
    field_nullable: bool,
    field_tri_state: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestStreamAbi {
    alias: String,
    item_type: String,
    error_type: String,
    lifetime: String,
    native_type: String,
    wasm_type: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestOperation {
    kind: ManifestOperationKind,
    source_operation: ManifestSourceOperation,
    emitted_operation_id: String,
    rust_method_name: String,
    parameters: Vec<ManifestParameter>,
    return_type: String,
    success_type: String,
    representation: ManifestRepresentation,
    success_statuses: Vec<String>,
    #[serde(default)]
    stream: Option<ManifestStreamAbi>,
    #[serde(default)]
    request_discriminators: Vec<ManifestRequestDiscriminator>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestRawClient {
    type_path: String,
    constructor: String,
    api_key_builder: String,
    base_url_builder: String,
    type_preludes: Vec<String>,
}

fn manifest_error(message: impl Into<String>) -> Error {
    Error::new(format!("invalid {MANIFEST_NAME}: {}", message.into()))
}

fn require_nonempty(value: &str, context: &str) -> Result<(), Error> {
    if value.is_empty() {
        Err(manifest_error(format!("{context} must not be empty")))
    } else {
        Ok(())
    }
}

fn qualify_generated_path(path: &str, context: &str) -> Result<String, Error> {
    require_nonempty(path, context)?;
    if path.starts_with("::")
        || path.starts_with("crate::")
        || path.starts_with("self::")
        || path.starts_with("super::")
        || path.split("::").any(str::is_empty)
    {
        return Err(manifest_error(format!(
            "{context} must be relative to the generated module root"
        )));
    }
    Ok(format!("crate::generated::{path}"))
}

fn to_json<T: Serialize>(value: &T, context: &str) -> Result<Value, Error> {
    serde_json::to_value(value)
        .map_err(|error| manifest_error(format!("failed to normalize {context}: {error}")))
}

/// Parse openapi-to-rust binding-manifest v1 into backend-neutral canonical Bindings v3.
pub fn parse_binding_manifest(source: &str) -> Result<Bindings, Error> {
    let manifest: Manifest = serde_json::from_str(source)
        .map_err(|error| manifest_error(format!("schema decode failed: {error}")))?;

    if manifest.schema != MANIFEST_SCHEMA {
        return Err(manifest_error(format!(
            "unsupported schema identifier {:?}",
            manifest.schema
        )));
    }
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(manifest_error(format!(
            "unsupported schema version {}",
            manifest.schema_version
        )));
    }
    if manifest.generator.name != "openapi-to-rust" {
        return Err(manifest_error(format!(
            "unexpected generator {:?}",
            manifest.generator.name
        )));
    }
    require_nonempty(&manifest.generator.version, "generator.version")?;

    let structs = manifest
        .structs
        .into_iter()
        .map(|(name, fields)| {
            let fields = fields
                .into_iter()
                .map(|field| {
                    json!({
                        "name": field.name,
                        "wire_name": field.wire_name,
                        "type": field.type_name,
                    })
                })
                .collect();
            (name, Value::Array(fields))
        })
        .collect::<Map<_, _>>();

    let enums = manifest
        .enums
        .into_iter()
        .map(|(name, variants)| {
            let variants = variants
                .into_iter()
                .map(|variant| {
                    json!({
                        "name": variant.name,
                        "payload": variant.payload,
                        "wire_name": variant.wire_name,
                    })
                })
                .collect();
            (name, Value::Array(variants))
        })
        .collect::<Map<_, _>>();

    let aliases = manifest
        .aliases
        .into_iter()
        .map(|(name, alias)| (name, Value::String(alias)))
        .collect::<Map<_, _>>();

    let mut symbol_paths = Map::new();
    for (name, path) in manifest.symbol_paths {
        symbol_paths.insert(
            name.clone(),
            Value::String(qualify_generated_path(
                &path,
                &format!("symbol_paths.{name}"),
            )?),
        );
    }

    let mut operations = Map::new();
    let mut canonical_identities = BTreeSet::new();
    for operation in manifest.operations {
        require_nonempty(&operation.rust_method_name, "operations[].rust_method_name")?;
        require_nonempty(
            &operation.source_operation.operation_id,
            "operations[].source_operation.operation_id",
        )?;
        require_nonempty(
            &operation.source_operation.method,
            "operations[].source_operation.method",
        )?;
        require_nonempty(
            &operation.source_operation.path,
            "operations[].source_operation.path",
        )?;
        require_nonempty(
            &operation.emitted_operation_id,
            "operations[].emitted_operation_id",
        )?;

        if operation.representation.is_streaming() != operation.stream.is_some() {
            return Err(manifest_error(format!(
                "operation {} stream ABI presence disagrees with representation",
                operation.rust_method_name
            )));
        }

        let identity = (
            operation.kind.clone(),
            operation.source_operation.clone(),
            operation.representation.clone(),
        );
        if !canonical_identities.insert(identity) {
            return Err(manifest_error(format!(
                "duplicate canonical source-operation/representation identity for {}",
                operation.rust_method_name
            )));
        }

        let mut seen_statuses = BTreeSet::new();
        if operation
            .success_statuses
            .iter()
            .any(|status| !seen_statuses.insert(status))
        {
            return Err(manifest_error(format!(
                "operation {} has duplicate success statuses",
                operation.rust_method_name
            )));
        }

        let common_stream = operation.stream.as_ref().map(|stream| {
            json!({
                "item_type": stream.item_type,
                "error_type": stream.error_type,
                "lifetime": stream.lifetime,
            })
        });

        let parameters = operation
            .parameters
            .iter()
            .map(|parameter| {
                json!({
                    "name": parameter.name,
                    "type": parameter.type_name,
                })
            })
            .collect::<Vec<_>>();

        let metadata = json!({
            "kind": to_json(&operation.kind, "operation kind")?,
            "source_operation": to_json(&operation.source_operation, "source operation")?,
            "emitted_operation_id": operation.emitted_operation_id,
            "representation": to_json(&operation.representation, "response representation")?,
            "success_statuses": operation.success_statuses,
            "request_discriminators": to_json(
                &operation.request_discriminators,
                "request discriminators",
            )?,
            "stream_abi": to_json(&operation.stream, "stream ABI")?,
        });

        let method_name = operation.rust_method_name;
        if operations.contains_key(&method_name) {
            return Err(manifest_error(format!(
                "duplicate generated Rust method name {method_name}"
            )));
        }
        operations.insert(
            method_name.clone(),
            json!({
                "name": method_name,
                "parameters": parameters,
                "return_type": operation.return_type,
                "success_type": operation.success_type,
                "stream": common_stream,
                "metadata": metadata,
            }),
        );
    }

    let client_type_path =
        qualify_generated_path(&manifest.raw_client.type_path, "raw_client.type_path")?;
    let mut type_preludes = Vec::with_capacity(manifest.raw_client.type_preludes.len());
    for (index, prelude) in manifest.raw_client.type_preludes.iter().enumerate() {
        type_preludes.push(qualify_generated_path(
            prelude,
            &format!("raw_client.type_preludes[{index}]"),
        )?);
    }

    Bindings::from_value(json!({
        "schema_version": 3,
        "structs": structs,
        "enums": enums,
        "aliases": aliases,
        "operations": operations,
        "symbol_paths": symbol_paths,
        "binding": {
            "client": {
                "type_path": client_type_path,
                "constructor": manifest.raw_client.constructor,
                "api_key_builder": manifest.raw_client.api_key_builder,
                "base_url_builder": manifest.raw_client.base_url_builder,
            },
            "type_preludes": type_preludes,
        },
    }))
}
