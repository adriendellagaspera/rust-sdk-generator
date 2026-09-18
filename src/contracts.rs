use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Published OpenAPI input. Indexing and normalization are separate compiler stages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpenApi(pub Value);

/// Backend-neutral, versioned Rust binding sidecar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bindings {
    pub schema_version: u32,
    pub structs: BTreeMap<String, Vec<FieldBinding>>,
    pub enums: BTreeMap<String, Vec<VariantBinding>>,
    pub aliases: BTreeMap<String, String>,
    pub operations: BTreeMap<String, OperationBinding>,
    pub symbol_paths: BTreeMap<String, String>,
    pub binding: BindingLayout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldBinding {
    pub name: String,
    #[serde(default)]
    pub wire_name: Option<String>,
    #[serde(rename = "type")]
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariantBinding {
    pub name: String,
    pub payload: Option<String>,
    pub wire_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterBinding {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamBinding {
    pub item_type: String,
    pub error_type: String,
    pub lifetime: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationBindingKind {
    CallShape,
    MultipartFilenames,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceOperationBinding {
    pub operation_id: String,
    pub method: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResponseRepresentationBinding {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestDiscriminatorValue {
    Bool(bool),
    Integer(i64),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestDiscriminatorBinding {
    pub wire_name: String,
    pub rust_access_path: Vec<String>,
    pub rust_value_type: String,
    pub value: RequestDiscriminatorValue,
    pub field_required: bool,
    pub field_nullable: bool,
    pub field_tri_state: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamAbiBinding {
    pub alias: String,
    pub item_type: String,
    pub error_type: String,
    pub lifetime: String,
    pub native_type: String,
    pub wasm_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationMetadataBinding {
    pub kind: OperationBindingKind,
    pub source_operation: SourceOperationBinding,
    pub emitted_operation_id: String,
    pub representation: ResponseRepresentationBinding,
    pub success_statuses: Vec<String>,
    #[serde(default)]
    pub request_discriminators: Vec<RequestDiscriminatorBinding>,
    #[serde(default)]
    pub stream_abi: Option<StreamAbiBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationBinding {
    pub name: String,
    pub parameters: Vec<ParameterBinding>,
    pub return_type: String,
    pub success_type: String,
    #[serde(default)]
    pub stream: Option<StreamBinding>,
    #[serde(default)]
    pub metadata: Option<OperationMetadataBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientBinding {
    pub type_path: String,
    pub constructor: String,
    pub api_key_builder: String,
    pub base_url_builder: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingLayout {
    pub client: ClientBinding,
    pub type_preludes: Vec<String>,
}

/// Explicit complete SDK definition accepted by generation today.
///
/// Automatic derivation of this contract is intentionally left to issue #1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkDefinition {
    pub schema_version: u32,
    pub client: ClientDefinition,
    pub models: IndexMap<String, ModelDefinition>,
    pub resources: IndexMap<String, ResourceDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientDefinition {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDefinition {
    #[serde(default)]
    pub raw: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub schema_path: Option<Vec<String>>,
    #[serde(default)]
    pub constructor: Option<Vec<String>>,
    #[serde(default)]
    pub exclude: Option<Vec<String>>,
    #[serde(default)]
    pub adapters: Option<IndexMap<String, String>>,
    #[serde(default)]
    pub union: Option<UnionDefinition>,
    #[serde(default)]
    pub simple_union: Option<SimpleUnionDefinition>,
    #[serde(default)]
    pub type_alias: Option<bool>,
    #[serde(default)]
    pub map: Option<MapDefinition>,
    #[serde(default)]
    pub scalar_enum: Option<ScalarEnumDefinition>,
    #[serde(default)]
    pub union_factory: Option<UnionFactoryDefinition>,
    #[serde(default)]
    pub borrowed: Option<bool>,
    #[serde(default)]
    pub accessors: Option<IndexMap<String, AccessorDefinition>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnionDefinition {
    pub root: String,
    pub path: Vec<String>,
    pub payload: String,
    #[serde(default)]
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimpleUnionDefinition {
    #[serde(default)]
    pub bidirectional: bool,
    pub variants: IndexMap<String, SimpleUnionVariant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SimpleUnionVariant {
    Name(String),
    Adapted { name: String, adapter: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapDefinition {
    pub root: String,
    pub path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScalarEnumDefinition {
    pub root: String,
    pub path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnionFactoryDefinition {
    pub field: String,
    #[serde(default)]
    pub leading: Vec<String>,
    #[serde(default)]
    pub rename: IndexMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessorKindDefinition {
    Copy,
    Ref,
    OptionalCopy,
    OptionalRef,
    Iter,
    FirstStringVariant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessorDefinition {
    pub kind: AccessorKindDefinition,
    pub path: Vec<String>,
    #[serde(default)]
    pub wrapper: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceDefinition {
    pub name: String,
    #[serde(default)]
    pub path: Option<Vec<String>>,
    pub operations: IndexMap<String, OperationDefinition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestMediaDefinition {
    Json,
    MultipartFormData,
    FormUrlencoded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationDefinition {
    pub operation_id: String,
    #[serde(default)]
    pub raw_method: Option<String>,
    #[serde(default)]
    pub request: Option<String>,
    #[serde(default)]
    pub request_media: Option<RequestMediaDefinition>,
    #[serde(default)]
    pub response: Option<String>,
    #[serde(default)]
    pub empty_response: Option<bool>,
    #[serde(default)]
    pub binary_response: Option<bool>,
    #[serde(default)]
    pub stream: Option<StreamDefinition>,
    #[serde(default)]
    pub request_overrides: Option<IndexMap<String, Option<bool>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamDefinition {
    pub item: String,
    #[serde(default)]
    pub wrapper: Option<String>,
    #[serde(rename = "type")]
    pub type_name: String,
}

/// Consumer-owned support paths referenced by emitted Rust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Runtime {
    pub error_type: String,
    pub error_module: String,
    pub error_exports: Vec<String>,
    pub sse_module: String,
    pub sse_function: String,
    pub generated_marker: String,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            error_type: "SdkError".into(),
            error_module: "error".into(),
            error_exports: vec![
                "ApiError".into(),
                "SdkError".into(),
                "TransportError".into(),
                "TransportErrorKind".into(),
            ],
            sse_module: "crate::streaming".into(),
            sse_function: "json_events".into(),
            generated_marker: "// @generated by rust-sdk-compiler; do not edit by hand.\n".into(),
        }
    }
}

/// Complete owned input for deterministic SDK generation.
///
/// Semantic derivation of `SdkDefinition` remains the responsibility of issue #1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateInput {
    pub openapi: OpenApi,
    pub bindings: Bindings,
    pub definition: SdkDefinition,
    #[serde(default)]
    pub runtime: Runtime,
}

/// Deterministic inventory of public names selected by lowering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiInventory {
    pub client: String,
    pub models: Vec<String>,
    pub resources: Vec<ResourceInventory>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceInventory {
    pub path: Vec<String>,
    pub module: String,
    pub name: String,
    pub operations: Vec<String>,
}

/// Generated files plus the public API inventory produced by the same lowering pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedSdk {
    pub files: BTreeMap<String, String>,
    pub inventory: ApiInventory,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_runtime_matches_existing_contract() {
        let runtime = Runtime::default();
        assert_eq!(runtime.error_type, "SdkError");
        assert_eq!(runtime.error_module, "error");
        assert_eq!(runtime.sse_module, "crate::streaming");
        assert_eq!(runtime.sse_function, "json_events");
    }

    #[test]
    fn bindings_maps_are_ordered() {
        let value = serde_json::json!({
            "schema_version": 2,
            "structs": {},
            "enums": {},
            "aliases": {},
            "operations": {},
            "symbol_paths": {},
            "binding": {
                "client": {
                    "type_path": "crate::raw::Client",
                    "constructor": "new",
                    "api_key_builder": "with_api_key",
                    "base_url_builder": "with_base_url"
                },
                "type_preludes": []
            }
        });
        let bindings: Bindings = serde_json::from_value(value).expect("valid bindings");
        assert_eq!(bindings.schema_version, 2);
    }

    #[test]
    fn sdk_definition_preserves_explicit_order() {
        let definition: SdkDefinition =
            serde_json::from_str(include_str!("../tests/fixtures/library/policy.json"))
                .expect("fixture definition");
        assert_eq!(
            definition
                .models
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["NewBook", "Book", "BookCollection"]
        );
        assert_eq!(
            definition.resources["catalog_books"]
                .operations
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["create", "list", "delete", "download"]
        );
    }
}
