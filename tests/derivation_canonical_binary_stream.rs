use std::collections::BTreeMap;

use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface,
    OperationOverride, ResponseRepresentationDefinition, Runtime, SdkOverrides, derive, generate,
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
    assert_eq!(
        operation.raw_method.as_deref(),
        Some("raw_archive_stream_41")
    );
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
        source.contains("pub async fn stream(&self) -> Result<BinaryStream, SdkError>")
            && source.contains("self.raw.raw_archive_stream_41(")
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
    assert_eq!(outcome.reason.code, "capability.binary_stream_abi_required");
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

    assert_eq!(error.diagnostic.code, "lower.response_representation_drift");
}

#[test]
fn explicit_binary_transport_selection_is_not_inferred_from_public_names() {
    let (mut openapi, mut bindings, mut surface) = fixture();
    let responses = &mut openapi.0["paths"]["/archives/export"]["get"]["responses"];
    responses["200"]["content"]["application/octet-stream"]["schema"] =
        serde_json::json!({"type": "string", "format": "binary"});
    let mut buffered = bindings.operations["raw_archive_stream_41"].clone();
    buffered.name = "raw_archive_buffered_42".into();
    buffered.return_type = "Result<bytes::Bytes, Error>".into();
    buffered.success_type = "bytes::Bytes".into();
    buffered.stream = None;
    let metadata = buffered.metadata.as_mut().expect("canonical metadata");
    metadata.representation = rust_sdk_generator::ResponseRepresentationBinding::BinaryBuffered {
        media_type: "application/octet-stream".into(),
        wildcard: false,
    };
    metadata.stream_abi = None;
    bindings.operations.insert(buffered.name.clone(), buffered);
    surface.operations.insert(
        "stream_archive".into(),
        vec!["archives.download".into(), "archives.live".into()],
    );

    let rejected = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface: surface.clone(),
        overrides: SdkOverrides::default(),
    })
    .expect("closed world must report ambiguity");
    assert_eq!(
        rejected.report.operations["stream_archive"].reason.code,
        "bindings.source_operation_identity_required"
    );

    let mut overrides = SdkOverrides::default();
    overrides.operations.insert(
        "stream_archive".into(),
        OperationOverride {
            request_overrides: BTreeMap::new(),
            response_representations: BTreeMap::from([
                (
                    "archives.download".into(),
                    ResponseRepresentationDefinition::BinaryBuffered,
                ),
                (
                    "archives.live".into(),
                    ResponseRepresentationDefinition::BinaryStream,
                ),
            ]),
        },
    );
    let derived = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides,
    })
    .expect("derive distinct binary transports");
    assert_eq!(
        derived.report.operations["stream_archive"].status,
        DerivationStatus::Overridden
    );
    assert_eq!(
        derived.definition.resources["archives"].operations["download"].raw_method.as_deref(),
        Some("raw_archive_buffered_42")
    );
    assert_eq!(
        derived.definition.resources["archives"].operations["live"].raw_method.as_deref(),
        Some("raw_archive_stream_41")
    );

    generate(GenerateInput {
        openapi,
        bindings,
        definition: derived.definition,
        runtime: Runtime::default(),
    })
    .expect("both binary transports lower");
}
