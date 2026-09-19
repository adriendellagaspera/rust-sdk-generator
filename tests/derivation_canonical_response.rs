use std::collections::BTreeMap;

use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, OperationOverride,
    PublicSdkSurface, ResponseRepresentationDefinition, Runtime, SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-response/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-response/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-response/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_and_generates_canonical_json_and_empty_representations() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let read = &derivation.report.operations["read_report"];
    assert_eq!(read.status, DerivationStatus::Derived);
    assert_eq!(read.reason.code, "inference.structurally_proven");
    assert_eq!(read.binding.as_deref(), Some("raw_read_report"));

    let marker = &derivation.report.operations["read_marker"];
    assert_eq!(marker.status, DerivationStatus::Derived);
    assert_eq!(marker.reason.code, "inference.structurally_proven");
    assert_eq!(marker.binding.as_deref(), Some("raw_read_marker"));
    let marker_response = &derivation.definition.models["MarkerReportsResponse"];
    assert_eq!(marker_response.raw.as_deref(), Some("Marker"));
    assert!(
        marker_response
            .accessors
            .as_ref()
            .is_some_and(indexmap::IndexMap::is_empty)
    );

    let purge = &derivation.report.operations["purge_reports"];
    assert_eq!(purge.status, DerivationStatus::Derived);
    assert_eq!(purge.reason.code, "inference.structurally_proven");
    assert_eq!(purge.binding.as_deref(), Some("raw_purge_reports"));

    let response = &derivation.definition.models["CurrentReportsResponse"];
    assert_eq!(response.raw.as_deref(), Some("Report"));
    assert_eq!(response.borrowed, Some(false));
    assert!(
        response
            .accessors
            .as_ref()
            .is_some_and(indexmap::IndexMap::is_empty)
    );

    let read_operation = &derivation.definition.resources["reports"].operations["current"];
    assert_eq!(
        read_operation.raw_method.as_deref(),
        Some("raw_read_report")
    );
    assert_eq!(
        read_operation.response.as_deref(),
        Some("CurrentReportsResponse")
    );
    assert_eq!(read_operation.empty_response, None);

    let purge_operation = &derivation.definition.resources["reports"].operations["purge"];
    assert_eq!(
        purge_operation.raw_method.as_deref(),
        Some("raw_purge_reports")
    );
    assert_eq!(purge_operation.response, None);
    assert_eq!(purge_operation.empty_response, Some(true));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    assert_eq!(generated.inventory.client, "ReportsClient");
    assert!(
        generated
            .inventory
            .models
            .contains(&"CurrentReportsResponse".to_owned())
    );
    assert!(
        generated
            .files
            .values()
            .any(|source| { source.contains("self.raw.raw_read_report(") })
    );
    assert!(
        generated
            .files
            .values()
            .any(|source| { source.contains("self.raw.raw_purge_reports(") })
    );
}

#[test]
fn complex_response_view_can_remain_opaque_when_root_fields_are_exact() {
    let (mut openapi, bindings, surface) = fixture();
    openapi
        .0
        .pointer_mut("/components/schemas/ReportDetails/properties")
        .and_then(serde_json::Value::as_object_mut)
        .expect("report details properties")
        .insert("opaque".into(), serde_json::json!({"type": "string"}));

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("opaque complex response view should derive");

    let outcome = &derivation.report.operations["read_report"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    let response = &derivation.definition.models["CurrentReportsResponse"];
    assert!(
        response
            .accessors
            .as_ref()
            .is_some_and(indexmap::IndexMap::is_empty)
    );

    generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("opaque complex response view should lower");
}

#[test]
fn lowering_revalidates_selected_json_statuses() {
    let (mut openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    *openapi
        .0
        .pointer_mut("/paths/~1reports~1current/get/responses/206/content/application~1json/schema")
        .expect("206 JSON schema") = serde_json::json!({
        "type": "object",
        "properties": {
            "different": {"type": "string"}
        }
    });

    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect_err("selected response drift must fail lowering");

    assert_eq!(error.diagnostic.code, "lower.response_representation_drift");
}

#[test]
fn lowering_revalidates_selected_empty_statuses() {
    let (mut openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    openapi
        .0
        .pointer_mut("/paths/~1reports~1cache/delete/responses/202")
        .and_then(serde_json::Value::as_object_mut)
        .expect("202 response")
        .insert(
            "content".into(),
            serde_json::json!({
                "application/json": {"schema": {"$ref": "#/components/schemas/Report"}}
            }),
        );

    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect_err("selected empty response drift must fail lowering");

    assert_eq!(error.diagnostic.code, "lower.empty_response_drift");
}

fn canonical_multi_representation_fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let (mut openapi, mut bindings, mut surface) = fixture();
    openapi.0["paths"]["/reports/current"]["get"]["responses"]["204"] =
        serde_json::json!({"description": "empty response"});

    let mut empty = bindings.operations["raw_purge_reports"].clone();
    empty.name = "raw_read_report_empty".into();
    let metadata = empty.metadata.as_mut().expect("canonical metadata");
    metadata.source_operation.operation_id = "read_report".into();
    metadata.source_operation.method = "GET".into();
    metadata.source_operation.path = "/reports/current".into();
    metadata.emitted_operation_id = "read_report".into();
    bindings
        .operations
        .insert("raw_read_report_empty".into(), empty);
    surface.operations.insert(
        "read_report".into(),
        vec!["reports.current".into(), "reports.current_empty".into()],
    );
    (openapi, bindings, surface)
}

fn multi_representation_overrides() -> SdkOverrides {
    let mut overrides = SdkOverrides::default();
    overrides.operations.insert(
        "read_report".into(),
        OperationOverride {
            request_overrides: BTreeMap::new(),
            response_representations: BTreeMap::from([
                (
                    "reports.current".into(),
                    ResponseRepresentationDefinition::Json,
                ),
                (
                    "reports.current_empty".into(),
                    ResponseRepresentationDefinition::Empty,
                ),
            ]),
        },
    );
    overrides
}

#[test]
fn multi_representation_operation_requires_explicit_transport_decisions() {
    let (openapi, bindings, surface) = canonical_multi_representation_fixture();
    let result = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("closed-world derivation");
    let operation = &result.report.operations["read_report"];
    assert_eq!(operation.status, DerivationStatus::Rejected);
    assert_eq!(
        operation.reason.code,
        "bindings.source_operation_identity_required"
    );
    assert!(operation.public_bindings.is_empty());
}

#[test]
fn selects_each_public_call_shape_by_canonical_representation() {
    let (openapi, bindings, surface) = canonical_multi_representation_fixture();
    let derived = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: multi_representation_overrides(),
    })
    .expect("explicit representation derivation");

    let operation = &derived.report.operations["read_report"];
    assert_eq!(operation.status, DerivationStatus::Overridden);
    assert_eq!(operation.binding, None);
    assert_eq!(
        operation.public_bindings,
        BTreeMap::from([
            ("reports.current".into(), "raw_read_report".into()),
            (
                "reports.current_empty".into(),
                "raw_read_report_empty".into()
            ),
        ])
    );
    let reports = &derived.definition.resources["reports"].operations;
    assert_eq!(
        reports["current"].raw_method.as_deref(),
        Some("raw_read_report")
    );
    assert_eq!(
        reports["current_empty"].raw_method.as_deref(),
        Some("raw_read_report_empty")
    );
    assert_eq!(reports["current_empty"].empty_response, Some(true));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derived.definition,
        runtime: Runtime::default(),
    })
    .expect("explicit representation lowering");
    assert!(
        generated
            .files
            .values()
            .any(|source| { source.contains("self.raw.raw_read_report_empty(") })
    );
}

#[test]
fn rejects_unknown_or_unavailable_public_transport_decisions() {
    let (openapi, bindings, surface) = canonical_multi_representation_fixture();
    let mut overrides = multi_representation_overrides();
    overrides
        .operations
        .get_mut("read_report")
        .expect("override")
        .response_representations
        .insert(
            "reports.unknown".into(),
            ResponseRepresentationDefinition::Text,
        );
    let error = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface: surface.clone(),
        overrides,
    })
    .expect_err("unknown public alias");
    assert_eq!(error.diagnostic.code, "overrides.unknown_public_path");

    let mut overrides = multi_representation_overrides();
    overrides
        .operations
        .get_mut("read_report")
        .expect("override")
        .response_representations
        .insert(
            "reports.current".into(),
            ResponseRepresentationDefinition::Text,
        );
    let error = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides,
    })
    .expect_err("no matching canonical transport");
    assert_eq!(error.diagnostic.code, "overrides.unapplied");
}

#[test]
fn lowering_refuses_selected_transport_drift() {
    let (mut openapi, bindings, surface) = canonical_multi_representation_fixture();
    let derived = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: multi_representation_overrides(),
    })
    .expect("derive");

    openapi.0["paths"]["/reports/current"]["get"]["responses"]["204"]["content"] = serde_json::json!({"application/json": {
        "schema": {"$ref": "#/components/schemas/Report"}
    }});
    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derived.definition,
        runtime: Runtime::default(),
    })
    .expect_err("selected empty representation drift");
    assert_eq!(error.diagnostic.code, "lower.empty_response_drift");
}
