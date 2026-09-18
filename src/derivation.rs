use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::contracts::{Bindings, ClientDefinition, OpenApi, SdkDefinition};
use crate::error::{Diagnostic, GenerationError};
use crate::naming::derive_public_paths;
use crate::openapi::OpenApiIndex;
use crate::projection::{insert_projection, project_operation};
use crate::reconcile::reconcile;
use crate::structural::request_optional_boolean_field;

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct OperationOverride {
    #[serde(default)]
    pub request_overrides: BTreeMap<String, Option<bool>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SdkOverrides {
    pub schema_version: u32,
    #[serde(default)]
    pub excluded_operations: BTreeMap<String, String>,
    #[serde(default)]
    pub operations: BTreeMap<String, OperationOverride>,
}

impl Default for SdkOverrides {
    fn default() -> Self {
        Self {
            schema_version: 1,
            excluded_operations: BTreeMap::new(),
            operations: BTreeMap::new(),
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
    pub public_path: Option<String>,
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
    for (operation_id, operation_override) in &overrides.operations {
        let path = format!("overrides.operations.{operation_id}");
        if !operation_ids.contains(operation_id) {
            return Err(DerivationError::at(
                "overrides.unknown_operation",
                path,
                format!("SdkOverrides references unknown operation {operation_id}"),
            ));
        }
        if overrides.excluded_operations.contains_key(operation_id) {
            return Err(DerivationError::at(
                "overrides.conflict",
                path,
                format!("operation {operation_id} cannot be both excluded and overridden"),
            ));
        }
        if operation_override.request_overrides.is_empty() {
            return Err(DerivationError::at(
                "overrides.empty_operation",
                path,
                "operation override must contain at least one decision",
            ));
        }
        if let Some(field) = operation_override
            .request_overrides
            .keys()
            .find(|field| field.trim().is_empty())
        {
            return Err(DerivationError::at(
                "overrides.invalid_request_override",
                format!("{path}.request_overrides.{field}"),
                "request override field must not be empty",
            ));
        }
    }
    Ok(())
}

fn apply_operation_override(
    index: &OpenApiIndex,
    bindings: &Bindings,
    definition: &mut SdkDefinition,
    operation_id: &str,
    operation_override: &OperationOverride,
) -> Result<(), DerivationError> {
    let location = definition.resources.iter().find_map(|(resource_name, resource)| {
        resource
            .operations
            .iter()
            .find(|(_, operation)| operation.operation_id == operation_id)
            .map(|(public_name, _)| (resource_name.clone(), public_name.clone()))
    });
    let Some((resource_name, public_name)) = location else {
        return Err(DerivationError::at(
            "overrides.unapplied",
            format!("overrides.operations.{operation_id}"),
            format!("operation {operation_id} was not projected into SdkDefinition"),
        ));
    };

    let request_name = definition.resources[&resource_name].operations[&public_name]
        .request
        .clone()
        .ok_or_else(|| {
            DerivationError::at(
                "overrides.invalid_request_override",
                format!("overrides.operations.{operation_id}.request_overrides"),
                format!("operation {operation_id} has no JSON request model"),
            )
        })?;
    let request_model = definition.models.get(&request_name).ok_or_else(|| {
        DerivationError::new(
            "derivation.request_model_missing",
            format!("projected request model {request_name} is missing"),
        )
    })?;
    if request_model.schema_path.as_ref().is_some_and(|path| !path.is_empty()) {
        return Err(DerivationError::at(
            "overrides.invalid_request_override",
            format!("overrides.operations.{operation_id}.request_overrides"),
            "operation request override requires the root request model",
        ));
    }
    let raw = request_model.raw.as_deref().unwrap_or(&request_name);
    let schema = request_model.schema.as_deref().unwrap_or(raw);

    for field in operation_override.request_overrides.keys() {
        if !request_optional_boolean_field(index, schema, raw, field, bindings) {
            return Err(DerivationError::at(
                "overrides.invalid_request_override",
                format!("overrides.operations.{operation_id}.request_overrides.{field}"),
                format!(
                    "request override requires optional non-null Boolean in OpenAPI and Option<bool> in Bindings: {raw}.{field}"
                ),
            ));
        }
    }

    definition
        .resources
        .get_mut(&resource_name)
        .expect("located resource")
        .operations
        .get_mut(&public_name)
        .expect("located operation")
        .request_overrides = Some(
        operation_override
            .request_overrides
            .iter()
            .map(|(field, value)| (field.clone(), *value))
            .collect(),
    );
    Ok(())
}

/// Derive a complete SDK definition and exhaustive operation report.
///
/// Public naming is selected independently from wire reconciliation. Surface
/// evidence may refine names but never selects a generated transport variant.
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
    let naming =
        derive_public_paths(&openapi, &surface).map_err(DerivationError::from_generation)?;
    let reconciliation =
        reconcile(&openapi, &bindings).map_err(DerivationError::from_generation)?;
    let index = OpenApiIndex::new(&openapi).map_err(DerivationError::from_generation)?;

    let mut public_path_counts = BTreeMap::new();
    for operation_id in &operation_ids {
        if overrides.excluded_operations.contains_key(operation_id) {
            continue;
        }
        let Some(named) = naming.get(operation_id) else {
            continue;
        };
        if named.reason.is_none()
            && let Some(path) = &named.public_path
        {
            *public_path_counts.entry(path.clone()).or_insert(0usize) += 1;
        }
    }

    let mut definition = SdkDefinition {
        schema_version: 2,
        client: ClientDefinition {
            name: surface.client.clone().unwrap_or_else(|| "Client".into()),
        },
        models: Default::default(),
        resources: Default::default(),
    };
    let mut operations = BTreeMap::new();
    for operation_id in operation_ids {
        let named = naming.get(&operation_id).ok_or_else(|| {
            DerivationError::new(
                "derivation.naming_missing",
                format!("operation {operation_id} has no naming decision"),
            )
        })?;
        let public_paths = named.evidence.clone();
        let public_path = named.public_path.clone();
        let mut outcome = if let Some(reason) = overrides.excluded_operations.get(&operation_id) {
            OperationDerivation {
                status: DerivationStatus::Excluded,
                reason: DerivationReason {
                    code: "override.excluded".into(),
                    detail: Some(reason.clone()),
                },
                public_paths,
                public_path,
                binding: None,
            }
        } else {
            let matched = reconciliation.get(&operation_id).ok_or_else(|| {
                DerivationError::new(
                    "derivation.reconciliation_missing",
                    format!("operation {operation_id} was not reconciled"),
                )
            })?;
            if matched.binding.is_none() {
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
                    public_path,
                    binding: None,
                }
            } else if let Some(reason) = named.reason {
                OperationDerivation {
                    status: DerivationStatus::Rejected,
                    reason: DerivationReason {
                        code: reason.into(),
                        detail: None,
                    },
                    public_paths,
                    public_path: None,
                    binding: matched.binding.clone(),
                }
            } else if public_path
                .as_ref()
                .and_then(|path| public_path_counts.get(path))
                .is_some_and(|count| *count > 1)
            {
                OperationDerivation {
                    status: DerivationStatus::Rejected,
                    reason: DerivationReason {
                        code: "surface.public_path_collision".into(),
                        detail: None,
                    },
                    public_paths,
                    public_path,
                    binding: matched.binding.clone(),
                }
            } else {
                let binding = matched.binding.as_deref().expect("matched binding");
                let path = public_path
                    .as_deref()
                    .expect("naming decision has public path");
                match project_operation(&index, &bindings, &operation_id, binding, path)
                    .and_then(|projected| insert_projection(&mut definition, projected))
                {
                    Ok(()) => OperationDerivation {
                        status: DerivationStatus::Derived,
                        reason: DerivationReason {
                            code: "inference.structurally_proven".into(),
                            detail: None,
                        },
                        public_paths,
                        public_path,
                        binding: matched.binding.clone(),
                    },
                    Err(reason) => OperationDerivation {
                        status: DerivationStatus::Rejected,
                        reason: DerivationReason {
                            code: reason.into(),
                            detail: None,
                        },
                        public_paths,
                        public_path,
                        binding: matched.binding.clone(),
                    },
                }
            }
        };
        if let Some(operation_override) = overrides.operations.get(&operation_id) {
            if outcome.status != DerivationStatus::Derived {
                return Err(DerivationError::at(
                    "overrides.unapplied",
                    format!("overrides.operations.{operation_id}"),
                    format!(
                        "operation {operation_id} could not accept overrides because generic derivation ended as {:?} ({})",
                        outcome.status, outcome.reason.code
                    ),
                ));
            }
            apply_operation_override(
                &index,
                &bindings,
                &mut definition,
                &operation_id,
                operation_override,
            )?;
            outcome.status = DerivationStatus::Overridden;
            outcome.reason = DerivationReason {
                code: "override.request_overrides".into(),
                detail: Some(
                    operation_override
                        .request_overrides
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(","),
                ),
            };
        }
        operations.insert(operation_id, outcome);
    }

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
    use crate::contracts::{
        BindingLayout, ClientBinding, GenerateInput, OperationBinding, ParameterBinding, Runtime,
    };

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
        assert_eq!(
            outcome.public_path.as_deref(),
            Some("weather.read_forecast")
        );
        assert_eq!(
            outcome.reason.code,
            "capability.response_model_derivation_required"
        );
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

    #[test]
    fn projects_reconciled_body_path_and_query_independent_of_binding_order() {
        let api: OpenApi = serde_json::from_str(include_str!(
            "../tests/fixtures/derivation-update/openapi.json"
        ))
        .expect("fixture OpenAPI");
        let raw: Bindings = serde_json::from_str(include_str!(
            "../tests/fixtures/derivation-update/rust-bindings.json"
        ))
        .expect("fixture bindings");
        let surface: PublicSdkSurface = serde_json::from_str(include_str!(
            "../tests/fixtures/derivation-update/surface.json"
        ))
        .expect("fixture surface");

        let derivation = derive(DeriveInput {
            openapi: api.clone(),
            bindings: raw.clone(),
            surface,
            overrides: SdkOverrides::default(),
        })
        .expect("derive");
        let outcome = &derivation.report.operations["revise_job"];
        assert_eq!(outcome.status, DerivationStatus::Derived);
        assert_eq!(outcome.reason.code, "inference.structurally_proven");
        assert_eq!(outcome.binding.as_deref(), Some("call_42"));

        let request = &derivation.definition.models["UpdateWorkJobsRequest"];
        assert_eq!(request.raw.as_deref(), Some("UpdateJobRequest"));
        assert_eq!(
            request.constructor.as_deref(),
            Some(&["title".to_owned()][..])
        );
        let operation = &derivation.definition.resources["work_jobs"].operations["update"];
        assert_eq!(operation.operation_id, "revise_job");
        assert_eq!(operation.raw_method.as_deref(), Some("call_42"));
        assert_eq!(operation.request.as_deref(), Some("UpdateWorkJobsRequest"));
        assert_eq!(operation.empty_response, Some(true));

        let generated = crate::generate(GenerateInput {
            openapi: api,
            bindings: raw,
            definition: derivation.definition,
            runtime: Runtime::default(),
        })
        .expect("derived definition generates");
        assert_eq!(generated.inventory.client, "WorkClient");
        assert_eq!(generated.inventory.resources.len(), 2);
        assert_eq!(generated.inventory.resources[1].path, vec!["work", "jobs"]);
        assert_eq!(generated.inventory.resources[1].operations, vec!["update"]);
    }
}
