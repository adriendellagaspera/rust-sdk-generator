use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::contracts::{Bindings, ClientDefinition, OpenApi, SdkDefinition};
use crate::error::{Diagnostic, GenerationError};
use crate::openapi::OpenApiIndex;
use crate::reconcile::reconcile;

/// Consumer-provided public resource/method naming evidence.
///
/// This contract never describes wire behavior. OpenAPI and normalized Bindings
/// remain authoritative for structural and transport decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicSdkSurface {
    pub schema_version: u32,
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub operations: BTreeMap<String, Vec<String>>,
}

impl Default for PublicSdkSurface {
    fn default() -> Self {
        Self {
            schema_version: 1,
            client: None,
            operations: BTreeMap::new(),
        }
    }
}

/// Partial consumer-authored decisions generic derivation must not guess.
///
/// The initial contract supports explicit exclusion. Further semantic override
/// fields are added only at the generic boundary; raw-generator method names are
/// deliberately not part of this contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkOverrides {
    pub schema_version: u32,
    #[serde(default)]
    pub excluded_operations: BTreeMap<String, String>,
}

impl Default for SdkOverrides {
    fn default() -> Self {
        Self {
            schema_version: 1,
            excluded_operations: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivationStatus {
    Derived,
    Overridden,
    Excluded,
    Rejected,
}

/// Stable machine-readable explanation for one closed-world outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivationReason {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationDerivation {
    pub status: DerivationStatus,
    pub reason: DerivationReason,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub public_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
}

/// Exhaustive classification of every relevant OpenAPI operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivationReport {
    pub schema_version: u32,
    pub operations: BTreeMap<String, OperationDerivation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeriveInput {
    pub openapi: OpenApi,
    pub bindings: Bindings,
    #[serde(default)]
    pub surface: PublicSdkSurface,
    #[serde(default)]
    pub overrides: SdkOverrides,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Derivation {
    pub definition: SdkDefinition,
    pub report: DerivationReport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivationError {
    pub diagnostic: Diagnostic,
}

impl DerivationError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            diagnostic: Diagnostic {
                code: code.into(),
                message: message.into(),
                path: None,
            },
        }
    }

    fn at(code: &'static str, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            diagnostic: Diagnostic {
                code: code.into(),
                message: message.into(),
                path: Some(path.into()),
            },
        }
    }

    fn from_generation(error: GenerationError) -> Self {
        Self {
            diagnostic: error.diagnostic,
        }
    }
}

impl fmt::Display for DerivationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.diagnostic.message)
    }
}

impl std::error::Error for DerivationError {}

fn operation_ids(openapi: &OpenApi) -> Result<BTreeSet<String>, DerivationError> {
    // Reuse the canonical OpenAPI validation/indexing pass before collecting the
    // deterministic operation set needed by the public report.
    OpenApiIndex::new(openapi).map_err(DerivationError::from_generation)?;

    let root = openapi.0.as_object().ok_or_else(|| {
        DerivationError::new("openapi.invalid", "OpenAPI document must be a JSON object")
    })?;
    let mut result = BTreeSet::new();
    if let Some(paths) = root.get("paths").and_then(serde_json::Value::as_object) {
        for path_item in paths.values().filter_map(serde_json::Value::as_object) {
            for method in [
                "get", "put", "post", "delete", "patch", "head", "options", "trace",
            ] {
                let Some(operation) = path_item.get(method).and_then(serde_json::Value::as_object)
                else {
                    continue;
                };
                if let Some(operation_id) = operation
                    .get("operationId")
                    .and_then(serde_json::Value::as_str)
                {
                    result.insert(operation_id.to_owned());
                }
            }
        }
    }
    Ok(result)
}

fn validate_evidence(
    operation_ids: &BTreeSet<String>,
    surface: &PublicSdkSurface,
    overrides: &SdkOverrides,
) -> Result<(), DerivationError> {
    if surface.schema_version != 1 {
        return Err(DerivationError::at(
            "surface.schema_version",
            "surface.schema_version",
            format!(
                "unsupported PublicSdkSurface schema version {}",
                surface.schema_version
            ),
        ));
    }
    if overrides.schema_version != 1 {
        return Err(DerivationError::at(
            "overrides.schema_version",
            "overrides.schema_version",
            format!(
                "unsupported SdkOverrides schema version {}",
                overrides.schema_version
            ),
        ));
    }

    for (operation_id, paths) in &surface.operations {
        if !operation_ids.contains(operation_id) {
            return Err(DerivationError::at(
                "surface.unknown_operation",
                format!("surface.operations.{operation_id}"),
                format!("PublicSdkSurface references unknown operation {operation_id}"),
            ));
        }
        if paths.iter().any(|path| path.trim().is_empty()) {
            return Err(DerivationError::at(
                "surface.invalid_path",
                format!("surface.operations.{operation_id}"),
                "public paths must not be empty",
            ));
        }
    }

    for (operation_id, reason) in &overrides.excluded_operations {
        if !operation_ids.contains(operation_id) {
            return Err(DerivationError::at(
                "overrides.unknown_operation",
                format!("overrides.excluded_operations.{operation_id}"),
                format!("SdkOverrides references unknown operation {operation_id}"),
            ));
        }
        if reason.trim().is_empty() {
            return Err(DerivationError::at(
                "overrides.invalid_exclusion",
                format!("overrides.excluded_operations.{operation_id}"),
                "exclusion reason must not be empty",
            ));
        }
    }
    Ok(())
}

/// Derive a complete SDK definition and exhaustive operation report.
///
/// Generic reconciliation is intentionally conservative: a binding is attached
/// only when OpenAPI request/response structure identifies one unique operation
/// without relying on a raw-generator method name. Later #1 slices replace the
/// projection rejection with SDK inference; transport-sensitive ambiguity stays
/// rejected until Bindings carries source-operation and representation identity.
pub fn derive(input: DeriveInput) -> Result<Derivation, DerivationError> {
    let DeriveInput {
        openapi,
        bindings,
        surface,
        overrides,
    } = input;

    bindings
        .validate()
        .map_err(DerivationError::from_generation)?;
    let operation_ids = operation_ids(&openapi)?;
    validate_evidence(&operation_ids, &surface, &overrides)?;
    let reconciliation = reconcile(&openapi, &bindings).map_err(DerivationError::from_generation)?;

    let mut operations = BTreeMap::new();
    for operation_id in operation_ids {
        let public_paths = surface
            .operations
            .get(&operation_id)
            .cloned()
            .unwrap_or_default();
        let outcome = if let Some(reason) = overrides.excluded_operations.get(&operation_id) {
            OperationDerivation {
                status: DerivationStatus::Excluded,
                reason: DerivationReason {
                    code: "override.excluded".into(),
                    detail: Some(reason.clone()),
                },
                public_paths,
                binding: None,
            }
        } else {
            let matched = reconciliation.get(&operation_id).ok_or_else(|| {
                DerivationError::new(
                    "derivation.reconciliation_missing",
                    format!("operation {operation_id} was not reconciled"),
                )
            })?;
            if let Some(binding) = &matched.binding {
                OperationDerivation {
                    status: DerivationStatus::Rejected,
                    reason: DerivationReason {
                        code: "capability.projection_not_implemented".into(),
                        detail: None,
                    },
                    public_paths,
                    binding: Some(binding.clone()),
                }
            } else {
                OperationDerivation {
                    status: DerivationStatus::Rejected,
                    reason: DerivationReason {
                        code: matched
                            .reason
                            .unwrap_or("bindings.no_structural_match")
                            .into(),
                        detail: None,
                    },
                    public_paths,
                    binding: None,
                }
            }
        };
        operations.insert(operation_id, outcome);
    }

    let definition = SdkDefinition {
        schema_version: 2,
        client: ClientDefinition {
            name: surface.client.unwrap_or_else(|| "Client".into()),
        },
        models: Default::default(),
        resources: Default::default(),
    };
    definition
        .validate()
        .map_err(DerivationError::from_generation)?;

    Ok(Derivation {
        definition,
        report: DerivationReport {
            schema_version: 1,
            operations,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{BindingLayout, ClientBinding, OperationBinding, ParameterBinding};

    fn openapi() -> OpenApi {
        OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Derivation contract", "version": "1"},
            "paths": {
                "/widgets": {
                    "get": {
                        "operationId": "list_widgets",
                        "responses": {"200": {"description": "ok"}}
                    },
                    "post": {
                        "operationId": "create_widget",
                        "responses": {"204": {"description": "ok"}}
                    }
                }
            }
        }))
    }

    fn bindings() -> Bindings {
        Bindings {
            schema_version: 2,
            structs: BTreeMap::new(),
            enums: BTreeMap::new(),
            aliases: BTreeMap::new(),
            operations: BTreeMap::new(),
            symbol_paths: BTreeMap::new(),
            binding: BindingLayout {
                client: ClientBinding {
                    type_path: "crate::raw::Client".into(),
                    constructor: "new".into(),
                    api_key_builder: "with_api_key".into(),
                    base_url_builder: "with_base_url".into(),
                },
                type_preludes: Vec::new(),
            },
        }
    }

    #[test]
    fn closed_world_report_classifies_every_operation() {
        let derivation = derive(DeriveInput {
            openapi: openapi(),
            bindings: bindings(),
            surface: PublicSdkSurface::default(),
            overrides: SdkOverrides::default(),
        })
        .expect("derive");

        assert_eq!(
            derivation
                .report
                .operations
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["create_widget", "list_widgets"]
        );
        assert!(
            derivation
                .report
                .operations
                .values()
                .all(|item| item.status == DerivationStatus::Rejected)
        );
    }

    #[test]
    fn explicit_exclusion_is_observable() {
        let mut overrides = SdkOverrides::default();
        overrides.excluded_operations.insert(
            "create_widget".into(),
            "consumer does not expose mutation".into(),
        );
        let derivation = derive(DeriveInput {
            openapi: openapi(),
            bindings: bindings(),
            surface: PublicSdkSurface::default(),
            overrides,
        })
        .expect("derive");
        let outcome = &derivation.report.operations["create_widget"];
        assert_eq!(outcome.status, DerivationStatus::Excluded);
        assert_eq!(outcome.reason.code, "override.excluded");
    }

    #[test]
    fn contracts_round_trip_deterministically() {
        let input = DeriveInput {
            openapi: openapi(),
            bindings: bindings(),
            surface: PublicSdkSurface::default(),
            overrides: SdkOverrides::default(),
        };
        let first = serde_json::to_vec(&derive(input.clone()).expect("first")).expect("serialize");
        let second = serde_json::to_vec(&derive(input).expect("second")).expect("serialize");
        assert_eq!(first, second);
        let decoded: Derivation = serde_json::from_slice(&first).expect("round trip");
        assert_eq!(
            serde_json::to_vec(&decoded).expect("serialize again"),
            first
        );
    }

    #[test]
    fn unique_structural_binding_is_reported_without_name_identity() {
        let api = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/weather/{city}": {"get": {
                    "operationId": "read_forecast",
                    "parameters": [
                        {"name": "city", "in": "path", "schema": {"type": "string"}},
                        {"name": "days", "in": "query", "schema": {"type": "integer"}}
                    ],
                    "responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Forecast"}}}}}
                }}
            }
        }));
        let mut raw = bindings();
        raw.operations.insert(
            "opaque_call".into(),
            OperationBinding {
                name: "opaque_call".into(),
                parameters: vec![
                    ParameterBinding {
                        name: "days".into(),
                        type_name: "Option<i64>".into(),
                    },
                    ParameterBinding {
                        name: "city".into(),
                        type_name: "impl AsRef<str>".into(),
                    },
                ],
                return_type: "Result<Forecast, Error>".into(),
                success_type: "Forecast".into(),
                stream: None,
            },
        );
        let derivation = derive(DeriveInput {
            openapi: api,
            bindings: raw,
            surface: PublicSdkSurface::default(),
            overrides: SdkOverrides::default(),
        })
        .expect("derive");
        let outcome = &derivation.report.operations["read_forecast"];
        assert_eq!(outcome.binding.as_deref(), Some("opaque_call"));
        assert_eq!(outcome.reason.code, "capability.projection_not_implemented");
    }

    #[test]
    fn transport_ambiguity_is_machine_readable() {
        let api = OpenApi(serde_json::json!({
            "openapi": "3.1.0",
            "paths": {
                "/archive": {"get": {
                    "operationId": "archive",
                    "responses": {"200": {"content": {
                        "application/json": {"schema": {"$ref": "#/components/schemas/Archive"}},
                        "application/octet-stream": {"schema": {"type": "string", "format": "binary"}}
                    }}}
                }}
            }
        }));
        let derivation = derive(DeriveInput {
            openapi: api,
            bindings: bindings(),
            surface: PublicSdkSurface::default(),
            overrides: SdkOverrides::default(),
        })
        .expect("derive");
        assert_eq!(
            derivation.report.operations["archive"].reason.code,
            "transport.source_operation_identity_required"
        );
    }
}
