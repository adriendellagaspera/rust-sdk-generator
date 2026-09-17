use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, SimpleUnionVariant, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!("fixtures/derivation-union/openapi.json"))
        .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-union/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!("fixtures/derivation-union/surface.json"))
        .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_structurally_proven_simple_union_response() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["lookup_item"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");
    assert_eq!(outcome.binding.as_deref(), Some("opaque_union_call"));

    let union = &derivation.definition.models["LookupSearchResponse"];
    assert_eq!(union.raw.as_deref(), Some("LookupPayload"));
    let simple = union.simple_union.as_ref().expect("simple union response");
    assert!(simple.bidirectional);
    assert_eq!(simple.variants.len(), 2);
    assert_eq!(
        simple.variants["Variant2"],
        SimpleUnionVariant::Adapted {
            name: "FoundItem".into(),
            adapter: "LookupSearchResponseFoundItem".into(),
        }
    );
    assert_eq!(
        simple.variants["Variant7"],
        SimpleUnionVariant::Adapted {
            name: "MissingItem".into(),
            adapter: "LookupSearchResponseMissingItem".into(),
        }
    );

    let found = &derivation.definition.models["LookupSearchResponseFoundItem"];
    assert_eq!(found.raw.as_deref(), Some("FoundItem"));
    assert_eq!(found.borrowed, Some(false));
    assert_eq!(
        found
            .accessors
            .as_ref()
            .expect("found view accessors")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["id", "score"]
    );

    let missing = &derivation.definition.models["LookupSearchResponseMissingItem"];
    assert_eq!(missing.raw.as_deref(), Some("MissingItem"));
    assert_eq!(missing.borrowed, Some(false));

    let operation = &derivation.definition.resources["search"].operations["lookup"];
    assert_eq!(operation.raw_method.as_deref(), Some("opaque_union_call"));
    assert_eq!(operation.response.as_deref(), Some("LookupSearchResponse"));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");
    assert_eq!(generated.inventory.client, "SearchClient");
    assert!(
        generated
            .inventory
            .models
            .contains(&"LookupSearchResponse".to_owned())
    );
    assert!(
        generated
            .inventory
            .models
            .contains(&"LookupSearchResponseFoundItem".to_owned())
    );
    assert!(
        generated
            .inventory
            .models
            .contains(&"LookupSearchResponseMissingItem".to_owned())
    );

    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("impl From<LookupSearchResponse> for LookupPayload"));
    assert!(types.contains("impl From<LookupPayload> for LookupSearchResponse"));
    assert!(types.contains("LookupPayload::Variant2(value) => Self::FoundItem(value.into())"));
    assert!(types.contains("LookupPayload::Variant7(value) => Self::MissingItem(value.into())"));
}

#[test]
fn rejects_union_payload_drift() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .enums
        .get_mut("LookupPayload")
        .expect("union binding")[0]
        .payload = Some("FoundItem".into());

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    let outcome = &derivation.report.operations["lookup_item"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "capability.response_union_derivation_required"
    );
}

#[test]
fn rejects_union_branch_shape_drift() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .structs
        .get_mut("FoundItem")
        .expect("branch binding")[0]
        .type_name = "String".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    let outcome = &derivation.report.operations["lookup_item"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "capability.response_union_derivation_required"
    );
}
