use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, SimpleUnionVariant, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-request-union/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-request-union/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-request-union/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_named_and_inline_nested_request_unions_structurally() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["create_delivery"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");
    assert_eq!(outcome.binding.as_deref(), Some("opaque_delivery_call"));

    let root = &derivation.definition.models["CreateDeliveryTargetsRequest"];
    assert_eq!(root.raw.as_deref(), Some("OpaqueDelivery7"));
    assert_eq!(
        root.constructor.as_deref(),
        Some(&["label".to_owned(), "destination".to_owned()][..])
    );
    let adapters = root.adapters.as_ref().expect("root adapters");
    assert_eq!(
        adapters["destination"],
        "CreateDeliveryTargetsRequestDestination"
    );
    assert_eq!(adapters["fallback"], "CreateDeliveryTargetsRequestFallback");

    let destination =
        &derivation.definition.models["CreateDeliveryTargetsRequestDestination"];
    assert_eq!(destination.schema.as_deref(), Some("DeliveryDestination"));
    assert_eq!(destination.schema_path, None);
    assert_eq!(destination.raw.as_deref(), Some("OpaqueDestination3"));
    let union = destination.simple_union.as_ref().expect("named union");
    assert!(!union.bidirectional);
    assert_eq!(
        union.variants["CaseA"],
        SimpleUnionVariant::Adapted {
            name: "EmailTarget".into(),
            adapter: "CreateDeliveryTargetsRequestDestinationEmailTarget".into(),
        }
    );
    assert_eq!(
        union.variants["CaseB"],
        SimpleUnionVariant::Adapted {
            name: "WebhookTarget".into(),
            adapter: "CreateDeliveryTargetsRequestDestinationWebhookTarget".into(),
        }
    );

    let fallback = &derivation.definition.models["CreateDeliveryTargetsRequestFallback"];
    assert_eq!(fallback.schema.as_deref(), Some("CreateDeliveryRequest"));
    assert_eq!(
        fallback.schema_path.as_deref(),
        Some(&["fallback".to_owned()][..])
    );
    assert_eq!(fallback.raw.as_deref(), Some("OpaqueFallback6"));
    assert!(!fallback
        .simple_union
        .as_ref()
        .expect("inline union")
        .bidirectional);

    assert_eq!(
        derivation.definition.models
            ["CreateDeliveryTargetsRequestDestinationEmailTarget"]
            .raw
            .as_deref(),
        Some("OpaqueEmail4")
    );
    assert_eq!(
        derivation.definition.models
            ["CreateDeliveryTargetsRequestDestinationWebhookTarget"]
            .raw
            .as_deref(),
        Some("OpaqueHook8")
    );

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("pub enum CreateDeliveryTargetsRequestDestination"));
    assert!(types.contains(
        "EmailTarget(CreateDeliveryTargetsRequestDestinationEmailTarget)"
    ));
    assert!(types.contains(
        "WebhookTarget(CreateDeliveryTargetsRequestDestinationWebhookTarget)"
    ));
    assert!(types.contains(
        "impl From<CreateDeliveryTargetsRequestDestination> for OpaqueDestination3"
    ));
    assert!(!types.contains(
        "impl From<OpaqueDestination3> for CreateDeliveryTargetsRequestDestination"
    ));
    assert!(types.contains(
        "pub fn fallback(mut self, fallback: impl Into<CreateDeliveryTargetsRequestFallback>)"
    ));
}

#[test]
fn rejects_ambiguous_request_union_branch_matching() {
    let (openapi, mut bindings, surface) = fixture();
    bindings.structs.insert(
        "OpaqueHook8".into(),
        vec![rust_sdk_generator::FieldBinding {
            name: "address".into(),
            type_name: "String".into(),
        }],
    );

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["create_delivery"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn lowering_revalidates_request_union_branch_drift() {
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
        .pointer_mut("/components/schemas/EmailTarget/properties/address")
        .expect("email address schema") = serde_json::json!({"type": "boolean"});

    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect_err("request union branch drift must fail lowering");
    assert_eq!(error.diagnostic.code, "lower.request_union_drift");
}
