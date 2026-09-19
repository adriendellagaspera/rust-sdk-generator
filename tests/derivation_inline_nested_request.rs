use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-nested-request/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-nested-request/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-nested-request/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_nested_inline_request_models_with_schema_paths() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["create_profile"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");
    assert_eq!(outcome.binding.as_deref(), Some("opaque_profile_call"));

    let root = &derivation.definition.models["CreateAccountsProfilesRequest"];
    assert_eq!(root.schema.as_deref(), Some("CreateProfileRequest"));
    assert_eq!(root.schema_path, None);
    assert_eq!(root.raw.as_deref(), Some("OpaqueProfile9"));
    assert_eq!(
        root.constructor.as_deref(),
        Some(&["name".to_owned(), "settings".to_owned()][..])
    );
    assert_eq!(
        root.adapters.as_ref().expect("root adapters")["settings"],
        "CreateAccountsProfilesRequestSettings"
    );

    let settings = &derivation.definition.models["CreateAccountsProfilesRequestSettings"];
    assert_eq!(settings.schema.as_deref(), Some("CreateProfileRequest"));
    assert_eq!(
        settings.schema_path.as_deref(),
        Some(&["settings".to_owned()][..])
    );
    assert_eq!(settings.raw.as_deref(), Some("OpaqueSettings4"));
    assert_eq!(
        settings.constructor.as_deref(),
        Some(&["mode".to_owned()][..])
    );
    assert_eq!(
        settings.adapters.as_ref().expect("settings adapters")["tuning"],
        "CreateAccountsProfilesRequestSettingsTuning"
    );

    let tuning = &derivation.definition.models["CreateAccountsProfilesRequestSettingsTuning"];
    assert_eq!(tuning.schema.as_deref(), Some("CreateProfileRequest"));
    assert_eq!(
        tuning.schema_path.as_deref(),
        Some(&["settings".to_owned(), "tuning".to_owned()][..])
    );
    assert_eq!(tuning.raw.as_deref(), Some("OpaqueTuning2"));
    assert_eq!(tuning.constructor.as_deref(), Some(&[][..]));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("settings: impl Into<CreateAccountsProfilesRequestSettings>"));
    assert!(types.contains(
        "pub fn tuning(mut self, tuning: impl Into<CreateAccountsProfilesRequestSettingsTuning>)"
    ));
    assert!(types.contains("self.raw.tuning = Some(Some("));
    assert!(types.contains("pub fn tuning_null(mut self) -> Self"));
    assert!(types.contains("self.raw.tuning = Some(None);"));
    assert!(
        types.contains(
            "pub struct CreateAccountsProfilesRequestSettingsTuning { raw: OpaqueTuning2 }"
        )
    );
}

#[test]
fn rejects_inline_nested_request_shape_drift_during_reconciliation() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .structs
        .get_mut("OpaqueTuning2")
        .expect("tuning binding")[0]
        .type_name = "Option<bool>".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["create_profile"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn lowering_revalidates_inline_schema_path_drift() {
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
        .pointer_mut("/components/schemas/CreateProfileRequest/properties/settings/properties")
        .and_then(serde_json::Value::as_object_mut)
        .expect("settings properties")
        .insert("extra".into(), serde_json::json!({"type": "string"}));

    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect_err("inline schema drift must fail lowering");
    assert_eq!(error.diagnostic.code, "lower.field_drift");
}

#[test]
fn preserves_structurally_proven_required_nullable_inline_object_as_raw_request() {
    let (mut openapi, mut bindings, surface) = fixture();
    let settings = openapi
        .0
        .pointer_mut("/components/schemas/CreateProfileRequest/properties/settings")
        .expect("settings schema")
        .clone();
    *openapi
        .0
        .pointer_mut("/components/schemas/CreateProfileRequest/properties/settings")
        .expect("settings schema") = serde_json::json!({
        "anyOf": [
            settings,
            {"type": "null"}
        ]
    });
    bindings
        .structs
        .get_mut("OpaqueProfile9")
        .expect("root binding")[0]
        .type_name = "Option<OpaqueSettings4>".into();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface: surface.clone(),
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["create_profile"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    let request = &derivation.definition.models["CreateAccountsProfilesRequest"];
    assert!(request.constructor.is_none());
    assert!(request.accessors.as_ref().is_some_and(indexmap::IndexMap::is_empty));
    assert!(generate(GenerateInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .is_ok());

    bindings
        .structs
        .get_mut("OpaqueProfile9")
        .expect("root binding")[0]
        .type_name = "OpaqueSettings4".into();
    let drifted = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("fail closed");
    assert_eq!(
        drifted.report.operations["create_profile"].status,
        DerivationStatus::Rejected
    );
}
