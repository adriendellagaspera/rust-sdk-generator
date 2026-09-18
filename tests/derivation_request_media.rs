use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface,
    RequestMediaDefinition, Runtime, SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-request-media/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-request-media/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-request-media/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_multipart_and_form_request_models_through_the_common_path() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    for operation_id in ["create_upload", "submit_form"] {
        let outcome = &derivation.report.operations[operation_id];
        assert_eq!(outcome.status, DerivationStatus::Derived);
        assert_eq!(outcome.reason.code, "inference.structurally_proven");
    }

    let upload =
        &derivation.definition.resources["media_uploads"].operations["create"];
    assert_eq!(upload.raw_method.as_deref(), Some("raw_upload_17"));
    assert_eq!(
        upload.request.as_deref(),
        Some("CreateMediaUploadsRequest")
    );
    assert_eq!(
        upload.request_media,
        Some(RequestMediaDefinition::MultipartFormData)
    );

    let upload_model = &derivation.definition.models["CreateMediaUploadsRequest"];
    assert_eq!(upload_model.raw.as_deref(), Some("OpaqueUpload5"));
    assert_eq!(
        upload_model.constructor.as_deref(),
        Some(
            &[
                "labels".to_owned(),
                "thumbnail".to_owned(),
                "publish".to_owned()
            ][..]
        )
    );

    let form = &derivation.definition.resources["media_forms"].operations["submit"];
    assert_eq!(form.raw_method.as_deref(), Some("raw_form_23"));
    assert_eq!(form.request.as_deref(), Some("SubmitMediaFormsRequest"));
    assert_eq!(
        form.request_media,
        Some(RequestMediaDefinition::FormUrlencoded)
    );

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    let types = &generated.files["facade_types.rs"];
    assert!(types.contains(
        "pub fn new(labels: Vec<String>, thumbnail: bytes::Bytes, publish: bool)"
    ));
    assert!(types.contains("pub fn file(mut self, file: bytes::Bytes)"));
    assert!(types.contains("pub fn file_null(mut self)"));
    assert!(types.contains("pub fn new(title: impl Into<String>, tags: Vec<String>)"));

    assert!(generated.files.values().any(|source| {
        source.contains("self.raw.raw_upload_17(")
            && source.contains("request.into_raw()")
            && source.contains("project_id")
            && source.contains("dry_run")
    }));
    assert!(generated.files.values().any(|source| {
        source.contains("self.raw.raw_form_23(")
            && source.contains("request.into_raw()")
            && source.contains("project_id")
            && source.contains("audit")
    }));
}

#[test]
fn rejects_multipart_binary_type_drift_during_reconciliation() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .structs
        .get_mut("OpaqueUpload5")
        .expect("upload binding")
        .iter_mut()
        .find(|field| field.name == "thumbnail")
        .expect("thumbnail field")
        .type_name = "bool".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["create_upload"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn lowering_revalidates_structured_request_media() {
    let (mut openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let content = openapi
        .0
        .pointer_mut("/paths/~1projects~1{project_id}~1uploads/post/requestBody/content")
        .and_then(serde_json::Value::as_object_mut)
        .expect("multipart content");
    let payload = content
        .remove("multipart/form-data")
        .expect("multipart payload");
    content.insert("application/x-www-form-urlencoded".into(), payload);

    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect_err("request media drift must fail lowering");

    assert_eq!(error.diagnostic.code, "lower.request_media_drift");
}

#[test]
fn unsupported_request_media_is_rejected_deterministically() {
    let (mut openapi, bindings, surface) = fixture();

    let content = openapi
        .0
        .pointer_mut("/paths/~1projects~1{project_id}~1uploads/post/requestBody/content")
        .and_then(serde_json::Value::as_object_mut)
        .expect("multipart content");
    let payload = content
        .remove("multipart/form-data")
        .expect("multipart payload");
    content.insert("text/csv".into(), payload);

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["create_upload"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "request.media_projection_unsupported");
}
