use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-array/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-array/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-array/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_inline_array_alias_without_schema_name_identity() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["read_inline_tags"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");
    assert_eq!(outcome.binding.as_deref(), Some("call_list_13"));

    let response = &derivation.definition.models["TagsReportsResponse"];
    assert_eq!(response.raw.as_deref(), Some("OpaqueList7"));
    assert_eq!(response.type_alias, Some(true));

    let operation = &derivation.definition.resources["reports"].operations["tags"];
    assert_eq!(operation.raw_method.as_deref(), Some("call_list_13"));
    assert_eq!(operation.response.as_deref(), Some("TagsReportsResponse"));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");
    assert_eq!(generated.inventory.client, "ReportsClient");
    assert_eq!(generated.inventory.models, vec!["TagsReportsResponse"]);
    assert_eq!(generated.inventory.resources[0].path, vec!["reports"]);
    assert_eq!(generated.inventory.resources[0].operations, vec!["tags"]);

    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("pub type TagsReportsResponse = Vec<String>;"));
}

#[test]
fn rejects_inline_array_alias_drift_during_reconciliation() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .aliases
        .insert("OpaqueList7".into(), "Vec<i64>".into());

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["read_inline_tags"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn rejects_ambiguous_inline_array_bindings_without_name_identity() {
    let (openapi, mut bindings, surface) = fixture();
    let mut duplicate = bindings.operations["call_list_13"].clone();
    duplicate.name = "read_inline_tags".into();
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

    let outcome = &derivation.report.operations["read_inline_tags"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "bindings.source_operation_identity_required"
    );
}
