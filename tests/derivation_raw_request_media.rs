use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface,
    RequestMediaDefinition, Runtime, SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-raw-request-media/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-raw-request-media/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-raw-request-media/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_raw_request_media_without_facade_models() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    for operation_id in ["put_blob", "replace_note", "upload_image"] {
        let outcome = &derivation.report.operations[operation_id];
        assert_eq!(outcome.status, DerivationStatus::Derived);
        assert_eq!(outcome.reason.code, "inference.structurally_proven");
    }

    let blob = &derivation.definition.resources["media_raw"].operations["put"];
    assert_eq!(blob.request, None);
    assert_eq!(
        blob.request_media,
        Some(RequestMediaDefinition::OctetStream)
    );
    assert_eq!(blob.raw_method.as_deref(), Some("opaque_blob_call"));

    let note = &derivation.definition.resources["media_text"].operations["replace"];
    assert_eq!(note.request, None);
    assert_eq!(note.request_media, Some(RequestMediaDefinition::TextPlain));

    let image = &derivation.definition.resources["media_images"].operations["upload"];
    assert_eq!(image.request, None);
    assert_eq!(image.request_media, Some(RequestMediaDefinition::Binary));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    assert!(generated.files.values().any(|source| {
        source.contains("self.raw.opaque_blob_call(")
            && source.contains("body: Option<bool>")
            && source.contains("request_body: Vec<u8>")
            && source.contains("opaque_blob_call(body, request_body, project_id.as_ref())")
    }));
    assert!(generated.files.values().any(|source| {
        source.contains("self.raw.opaque_note_call(")
            && source.contains("body: Option<String>")
            && source.contains("opaque_note_call(body, note_id.as_ref())")
    }));
    assert!(generated.files.values().any(|source| {
        source.contains("self.raw.opaque_image_call(")
            && source.contains("body: Vec<u8>")
            && source.contains("opaque_image_call(image_id.as_ref(), body)")
    }));
}

#[test]
fn rejects_raw_body_type_drift_during_reconciliation() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .operations
        .get_mut("opaque_blob_call")
        .expect("blob binding")
        .parameters
        .iter_mut()
        .find(|parameter| parameter.name == "opaque_payload9")
        .expect("raw body parameter")
        .type_name = "String".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["put_blob"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn lowering_revalidates_raw_media_and_optional_body_shape() {
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
        .pointer_mut("/paths/~1notes~1{note_id}/put/requestBody/required")
        .expect("request body required")
        .clone_from(&serde_json::Value::Bool(true));

    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect_err("raw body optionality drift must fail");

    assert_eq!(error.diagnostic.code, "lower.signature_drift");
}

#[test]
fn rejects_schema_less_and_non_binary_custom_media_deterministically() {
    let (mut openapi, bindings, surface) = fixture();

    let payload = openapi
        .0
        .pointer_mut("/paths/~1images~1{image_id}/post/requestBody/content/image~1png")
        .and_then(serde_json::Value::as_object_mut)
        .expect("image payload");
    payload.insert("schema".into(), serde_json::json!({"type": "string"}));

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["upload_image"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "request.media_projection_unsupported");
}
