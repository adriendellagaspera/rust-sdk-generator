use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface,
    ResponseRepresentationDefinition, Runtime, SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-buffered-response/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-buffered-response/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-buffered-response/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_and_generates_text_and_buffered_binary_responses() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    for operation_id in ["read_message", "read_blob"] {
        let outcome = &derivation.report.operations[operation_id];
        assert_eq!(outcome.status, DerivationStatus::Derived);
        assert_eq!(outcome.reason.code, "inference.structurally_proven");
    }

    let message = &derivation.definition.resources["payload"].operations["message"];
    assert_eq!(message.raw_method.as_deref(), Some("raw_read_message"));
    assert_eq!(
        message.response_representation,
        Some(ResponseRepresentationDefinition::Text)
    );
    assert_eq!(message.response, None);
    assert_eq!(message.empty_response, None);
    assert_eq!(message.binary_response, None);
    assert_eq!(message.stream, None);

    let blob = &derivation.definition.resources["payload"].operations["blob"];
    assert_eq!(blob.raw_method.as_deref(), Some("raw_read_blob"));
    assert_eq!(
        blob.response_representation,
        Some(ResponseRepresentationDefinition::BinaryBuffered)
    );
    assert_eq!(blob.response, None);
    assert_eq!(blob.binary_response, None);
    assert_eq!(blob.stream, None);

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    assert!(generated.files.values().any(|source| {
        source.contains(
            "pub async fn message(&self) -> Result<String, SdkError>"
        ) && source.contains("self.raw.raw_read_message(")
    }));
    assert!(generated.files.values().any(|source| {
        source.contains(
            "pub async fn blob(&self) -> Result<bytes::Bytes, SdkError>"
        ) && source.contains("self.raw.raw_read_blob(")
    }));
}

#[test]
fn lowering_revalidates_text_response_shape() {
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
        .pointer_mut("/paths/~1payload~1message/get/responses/206/content/text~1plain/schema")
        .expect("206 text schema") = serde_json::json!({
        "type": "string",
        "format": "binary"
    });

    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect_err("text response drift must fail lowering");

    assert_eq!(
        error.diagnostic.code,
        "lower.response_representation_drift"
    );
}

#[test]
fn lowering_revalidates_buffered_binary_response_shape() {
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
        .pointer_mut(
            "/paths/~1payload~1blob/get/responses/200/content/application~1octet-stream/schema",
        )
        .expect("binary schema") = serde_json::json!({"type": "string"});

    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect_err("buffered binary drift must fail lowering");

    assert_eq!(
        error.diagnostic.code,
        "lower.response_representation_drift"
    );
}
