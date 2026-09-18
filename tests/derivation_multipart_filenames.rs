use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface,
    RequestMediaDefinition, Runtime, SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-multipart-filenames/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-multipart-filenames/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-multipart-filenames/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_canonical_multipart_filename_helper_without_raw_name_heuristics() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["upload_asset"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");
    assert_eq!(outcome.binding.as_deref(), Some("raw_upload_call_17"));

    let operation = &derivation.definition.resources["assets"].operations["upload"];
    assert_eq!(operation.raw_method.as_deref(), Some("raw_upload_call_17"));
    assert_eq!(
        operation.request_media,
        Some(RequestMediaDefinition::MultipartFormData)
    );
    assert_eq!(operation.multipart_filenames, Some(true));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    let source = &generated.files["assets.rs"];
    assert!(source.contains("pub async fn upload(&self"));
    assert!(source.contains("self.raw.raw_upload_call_17("));
    assert!(source.contains("pub async fn upload_with_filenames(&self"));
    assert!(source.contains("multipart_filenames: &[(&str, &str)]"));
    assert!(source.contains("self.raw.zeta_aux_4("));
    assert!(source.contains(
        "self.raw.zeta_aux_4(asset_id.as_ref(), multipart_filenames, request.into_raw(), overwrite)"
    ));
}

#[test]
fn rejects_filename_helper_signature_drift_during_projection() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .operations
        .get_mut("zeta_aux_4")
        .expect("filename helper")
        .parameters
        .iter_mut()
        .find(|parameter| parameter.name == "multipart_filenames")
        .expect("filename parameter")
        .type_name = "&[(&str, String)]".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["upload_asset"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "bindings.multipart_filenames_signature_mismatch"
    );
}

#[test]
fn omits_filename_method_when_canonical_helper_is_absent() {
    let (openapi, mut bindings, surface) = fixture();
    bindings.operations.remove("zeta_aux_4");

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let operation = &derivation.definition.resources["assets"].operations["upload"];
    assert_eq!(operation.multipart_filenames, None);

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");
    assert!(!generated.files["assets.rs"].contains("upload_with_filenames"));
}
