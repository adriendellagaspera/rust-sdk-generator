use rust_sdk_generator::{
    AccessorKindDefinition, Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi,
    PublicSdkSurface, Runtime, SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-object/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-object/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-object/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_inline_scalar_object_without_schema_name_identity() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["read_snapshot"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");
    assert_eq!(outcome.binding.as_deref(), Some("call_result_31"));

    let response = &derivation.definition.models["SnapshotReportsResponse"];
    assert_eq!(response.raw.as_deref(), Some("OpaquePayload9"));
    assert_eq!(response.borrowed, Some(false));
    let accessors = response.accessors.as_ref().expect("response accessors");
    assert_eq!(accessors["attempts"].kind, AccessorKindDefinition::Copy);
    assert_eq!(accessors["note"].kind, AccessorKindDefinition::OptionalRef);
    assert_eq!(accessors["ready"].kind, AccessorKindDefinition::Copy);
    assert_eq!(accessors["title"].kind, AccessorKindDefinition::Ref);

    let operation = &derivation.definition.resources["reports"].operations["snapshot"];
    assert_eq!(operation.raw_method.as_deref(), Some("call_result_31"));
    assert_eq!(
        operation.response.as_deref(),
        Some("SnapshotReportsResponse")
    );

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");
    assert_eq!(generated.inventory.client, "ReportsClient");
    assert_eq!(generated.inventory.models, vec!["SnapshotReportsResponse"]);
    assert_eq!(generated.inventory.resources[0].path, vec!["reports"]);
    assert_eq!(
        generated.inventory.resources[0].operations,
        vec!["snapshot"]
    );

    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("pub struct SnapshotReportsResponse { raw: OpaquePayload9 }"));
    assert!(types.contains("impl From<OpaquePayload9> for SnapshotReportsResponse"));
}

#[test]
fn rejects_inline_object_field_drift_during_reconciliation() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .structs
        .get_mut("OpaquePayload9")
        .expect("inline response binding")[2]
        .type_name = "i64".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    let outcome = &derivation.report.operations["read_snapshot"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn rejects_ambiguous_inline_object_bindings_without_name_identity() {
    let (openapi, mut bindings, surface) = fixture();
    let mut duplicate = bindings.operations["call_result_31"].clone();
    duplicate.name = "read_snapshot".into();
    bindings
        .operations
        .insert("looks_like_operation_id".into(), duplicate);

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    let outcome = &derivation.report.operations["read_snapshot"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "bindings.source_operation_identity_required"
    );
}
