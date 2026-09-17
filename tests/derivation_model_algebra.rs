use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!("fixtures/derivation-models/openapi.json"))
        .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-models/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!("fixtures/derivation-models/surface.json"))
        .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_structurally_proven_model_algebra_responses() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    for operation_id in ["read_labels", "read_mode", "read_tags"] {
        let outcome = &derivation.report.operations[operation_id];
        assert_eq!(outcome.status, DerivationStatus::Derived);
        assert_eq!(outcome.reason.code, "inference.structurally_proven");
    }

    let labels = &derivation.definition.models["LabelsCatalogMetadataResponse"];
    assert_eq!(labels.raw.as_deref(), Some("LabelMap"));
    let map = labels.map.as_ref().expect("map response model");
    assert_eq!(map.root, "LabelMap");
    assert!(map.path.is_empty());

    let mode = &derivation.definition.models["ModeCatalogMetadataResponse"];
    assert_eq!(mode.raw.as_deref(), Some("RunMode"));
    let scalar_enum = mode.scalar_enum.as_ref().expect("scalar enum response model");
    assert_eq!(scalar_enum.root, "RunMode");
    assert!(scalar_enum.path.is_empty());

    let tags = &derivation.definition.models["TagsCatalogMetadataResponse"];
    assert_eq!(tags.raw.as_deref(), Some("TagList"));
    assert_eq!(tags.type_alias, Some(true));

    let metadata = &derivation.definition.resources["catalog_metadata"];
    assert_eq!(
        metadata.operations["labels"].response.as_deref(),
        Some("LabelsCatalogMetadataResponse")
    );
    assert_eq!(
        metadata.operations["mode"].response.as_deref(),
        Some("ModeCatalogMetadataResponse")
    );
    assert_eq!(
        metadata.operations["tags"].response.as_deref(),
        Some("TagsCatalogMetadataResponse")
    );

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");
    assert_eq!(generated.inventory.client, "CatalogClient");
    assert_eq!(
        generated.inventory.models,
        vec![
            "LabelsCatalogMetadataResponse",
            "ModeCatalogMetadataResponse",
            "TagsCatalogMetadataResponse"
        ]
    );
    assert_eq!(generated.inventory.resources.len(), 2);
    assert_eq!(
        generated.inventory.resources[1].path,
        vec!["catalog", "metadata"]
    );
    assert_eq!(
        generated.inventory.resources[1].operations,
        vec!["labels", "mode", "tags"]
    );
    let facade = &generated.files["facade_types.rs"];
    assert!(facade.contains("impl From<RunMode> for ModeCatalogMetadataResponse"));
    assert!(facade.contains("impl From<ModeCatalogMetadataResponse> for RunMode"));
}

#[test]
fn rejects_alias_response_type_drift() {
    let (openapi, mut bindings, surface) = fixture();
    bindings.aliases.insert("TagList".into(), "Vec<i64>".into());

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    let outcome = &derivation.report.operations["read_tags"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "capability.response_model_derivation_required"
    );
}

#[test]
fn rejects_map_response_value_drift() {
    let (openapi, mut bindings, surface) = fixture();
    bindings.structs.get_mut("LabelMap").expect("map binding")[0].type_name =
        "std::collections::BTreeMap<String, i64>".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    let outcome = &derivation.report.operations["read_labels"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "capability.response_model_derivation_required"
    );
}

#[test]
fn rejects_scalar_enum_wire_drift() {
    let (openapi, mut bindings, surface) = fixture();
    bindings.enums.get_mut("RunMode").expect("enum binding")[1].wire_name = Some("safer".into());

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    let outcome = &derivation.report.operations["read_mode"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "capability.response_model_derivation_required"
    );
}
