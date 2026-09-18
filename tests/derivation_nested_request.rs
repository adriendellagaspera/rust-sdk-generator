use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-nested-request/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-nested-request/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-nested-request/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_nested_named_request_models_without_raw_name_identity() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["create_widget"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");
    assert_eq!(outcome.binding.as_deref(), Some("opaque_create_call"));

    let root = &derivation.definition.models["CreatePlatformWidgetsRequest"];
    assert_eq!(root.schema.as_deref(), Some("CreateWidgetRequest"));
    assert_eq!(root.raw.as_deref(), Some("OpaqueRequest9"));
    assert_eq!(
        root.constructor.as_deref(),
        Some(&["name".to_owned(), "config".to_owned()][..])
    );
    let adapters = root.adapters.as_ref().expect("nested adapters");
    assert_eq!(adapters["config"], "CreatePlatformWidgetsRequestConfig");
    assert_eq!(adapters["metadata"], "CreatePlatformWidgetsRequestMetadata");

    let config = &derivation.definition.models["CreatePlatformWidgetsRequestConfig"];
    assert_eq!(config.schema.as_deref(), Some("WidgetConfig"));
    assert_eq!(config.raw.as_deref(), Some("OpaqueConfig4"));
    assert_eq!(
        config.constructor.as_deref(),
        Some(&["mode".to_owned()][..])
    );

    let metadata = &derivation.definition.models["CreatePlatformWidgetsRequestMetadata"];
    assert_eq!(metadata.schema.as_deref(), Some("WidgetMetadata"));
    assert_eq!(metadata.raw.as_deref(), Some("OpaqueMeta7"));
    assert_eq!(metadata.constructor.as_deref(), Some(&[][..]));

    let operation = &derivation.definition.resources["platform_widgets"].operations["create"];
    assert_eq!(operation.raw_method.as_deref(), Some("opaque_create_call"));
    assert_eq!(
        operation.request.as_deref(),
        Some("CreatePlatformWidgetsRequest")
    );
    assert_eq!(operation.empty_response, Some(true));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    assert_eq!(generated.inventory.client, "PlatformClient");
    for expected in [
        "CreatePlatformWidgetsRequest",
        "CreatePlatformWidgetsRequestConfig",
        "CreatePlatformWidgetsRequestMetadata",
    ] {
        assert!(generated.inventory.models.contains(&expected.to_owned()));
    }

    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("pub struct CreatePlatformWidgetsRequest { raw: OpaqueRequest9 }"));
    assert!(types.contains("config: impl Into<CreatePlatformWidgetsRequestConfig>"));
    assert!(types.contains(
        "pub fn metadata(mut self, metadata: impl Into<CreatePlatformWidgetsRequestMetadata>)"
    ));
    assert!(types.contains("pub struct CreatePlatformWidgetsRequestConfig { raw: OpaqueConfig4 }"));
    assert!(types.contains("pub struct CreatePlatformWidgetsRequestMetadata { raw: OpaqueMeta7 }"));
}

#[test]
fn rejects_nested_request_shape_drift_during_reconciliation() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .structs
        .get_mut("OpaqueConfig4")
        .expect("config binding")[1]
        .type_name = "bool".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["create_widget"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn rejects_recursive_named_request_shapes_deterministically() {
    let (mut openapi, mut bindings, surface) = fixture();
    let metadata = openapi
        .0
        .pointer_mut("/components/schemas/WidgetMetadata")
        .and_then(serde_json::Value::as_object_mut)
        .expect("metadata schema");
    metadata.insert("required".into(), serde_json::json!(["parent"]));
    metadata.insert(
        "properties".into(),
        serde_json::json!({
            "parent": {"$ref": "#/components/schemas/CreateWidgetRequest"}
        }),
    );
    bindings.structs.insert(
        "OpaqueMeta7".into(),
        vec![rust_sdk_generator::FieldBinding {
            name: "parent".into(),
            wire_name: None,
            type_name: "OpaqueRequest9".into(),
        }],
    );

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["create_widget"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}
