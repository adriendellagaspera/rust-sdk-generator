use rust_sdk_generator::{
    AccessorKindDefinition, Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi,
    PublicSdkSurface, Runtime, SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-array-object/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-array-object/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-array-object/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_inline_array_of_objects_as_collection_and_item_views() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["read_rows"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");
    assert_eq!(outcome.binding.as_deref(), Some("call_rows_28"));

    let collection = &derivation.definition.models["RowsReportsResponse"];
    assert_eq!(collection.raw.as_deref(), Some("OpaqueRows4"));
    assert_eq!(collection.borrowed, Some(false));
    assert_eq!(collection.type_alias, None);
    let collection_accessors = collection
        .accessors
        .as_ref()
        .expect("collection response accessors");
    assert_eq!(
        collection_accessors["iter"].kind,
        AccessorKindDefinition::Iter
    );
    assert!(collection_accessors["iter"].path.is_empty());
    assert_eq!(
        collection_accessors["iter"].wrapper.as_deref(),
        Some("RowsReportsResponseItem")
    );

    let item = &derivation.definition.models["RowsReportsResponseItem"];
    assert_eq!(item.raw.as_deref(), Some("OpaqueRow9"));
    assert_eq!(item.borrowed, Some(true));
    assert_eq!(
        item.accessors
            .as_ref()
            .expect("item accessors")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["enabled", "name", "note"]
    );

    let operation = &derivation.definition.resources["reports"].operations["rows"];
    assert_eq!(operation.raw_method.as_deref(), Some("call_rows_28"));
    assert_eq!(operation.response.as_deref(), Some("RowsReportsResponse"));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    assert_eq!(
        generated.inventory.models,
        vec!["RowsReportsResponseItem", "RowsReportsResponse"]
    );
    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("pub struct RowsReportsResponse { raw: OpaqueRows4 }"));
    assert!(types.contains(
        "pub fn iter(&self) -> impl ExactSizeIterator<Item = RowsReportsResponseItem<'_>>"
    ));
    assert!(types.contains("self.raw.iter().map(RowsReportsResponseItem::new)"));
    assert!(types.contains("pub struct RowsReportsResponseItem<'a> { raw: &'a OpaqueRow9 }"));
    assert!(types.contains("impl From<OpaqueRows4> for RowsReportsResponse"));
}

#[test]
fn rejects_inline_array_object_field_drift_during_reconciliation() {
    let (openapi, mut bindings, surface) = fixture();
    bindings.structs.get_mut("OpaqueRow9").expect("row binding")[1].type_name = "String".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["read_rows"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn rejects_ambiguous_inline_array_object_bindings_without_name_identity() {
    let (openapi, mut bindings, surface) = fixture();
    let mut duplicate = bindings.operations["call_rows_28"].clone();
    duplicate.name = "read_rows".into();
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

    let outcome = &derivation.report.operations["read_rows"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "bindings.source_operation_identity_required"
    );
}


#[test]
fn rejects_collection_iter_wrapper_with_wrong_raw_item() {
    let (openapi, bindings, surface) = fixture();
    let mut derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    derivation
        .definition
        .models
        .get_mut("RowsReportsResponse")
        .expect("collection response")
        .accessors
        .as_mut()
        .expect("collection accessors")
        .get_mut("iter")
        .expect("iter accessor")
        .wrapper = Some("RowsReportsResponse".into());

    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect_err("invalid iter wrapper must fail closed");

    assert_eq!(error.diagnostic.code, "lower.iter_wrapper");
}
