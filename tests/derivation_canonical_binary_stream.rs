use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface,
    ResponseRepresentationDefinition, Runtime, SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-binary-stream/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-binary-stream/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-binary-stream/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_canonical_binary_stream_from_binding_representation_and_abi() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["stream_archive"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");
    assert_eq!(outcome.binding.as_deref(), Some("raw_archive_stream_41"));

    let operation = &derivation.definition.resources["archives"].operations["stream"];
    assert_eq!(operation.raw_method.as_deref(), Some("raw_archive_stream_41"));
    assert_eq!(
        operation.response_representation,
        Some(ResponseRepresentationDefinition::BinaryStream)
    );
    assert_eq!(operation.binary_response, Some(true));
    assert_eq!(operation.response, None);
    assert_eq!(operation.empty_response, None);
    assert_eq!(operation.stream, None);

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    let types = &generated.files["facade_types.rs"];
    assert!(types.contains(
        "pub type BinaryStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes, SdkError>> + Send + 'static>>;"
    ));
    assert!(generated.files.values().any(|source| {
        source.contains(
            "pub async fn stream(&self) -> Result<BinaryStream, SdkError>"
        ) && source.contains("self.raw.raw_archive_stream_41(")
            && source.contains("chunk.map_err(Into::into)")
    }));
}

#[test]
fn rejects_canonical_binary_stream_with_non_byte_abi() {
    let (openapi, mut bindings, surface) = fixture();
    let operation = bindings
        .operations
        .get_mut("raw_archive_stream_41")
        .expect("binary stream binding");
    operation
        .stream
        .as_mut()
        .expect("legacy/common stream view")
        .item_type = "String".into();
    operation
        .metadata
        .as_mut()
        .and_then(|metadata| metadata.stream_abi.as_mut())
        .expect("canonical stream ABI")
        .item_type = "String".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["stream_archive"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "capability.binary_stream_abi_required"
    );
}

#[test]
fn lowering_revalidates_all_selected_binary_stream_statuses() {
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
            "/paths/~1archives~1export/get/responses/206/content/application~1octet-stream/schema",
        )
        .expect("206 binary schema") = serde_json::json!({"type": "string"});

    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect_err("binary stream response drift must fail lowering");

    assert_eq!(
        error.diagnostic.code,
        "lower.response_representation_drift"
    );
}
