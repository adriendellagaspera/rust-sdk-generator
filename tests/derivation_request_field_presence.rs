use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, SerializedPresenceBinding, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-request-field-presence/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-request-field-presence/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-request-field-presence/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

fn derive_fixture(
    openapi: OpenApi,
    bindings: Bindings,
    surface: PublicSdkSurface,
) -> rust_sdk_generator::Derivation {
    derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive")
}

#[test]
fn distinguishes_presence_from_nullability_for_request_fields() {
    let (openapi, bindings, surface) = fixture();
    let derivation = derive_fixture(openapi.clone(), bindings.clone(), surface);

    let outcome = &derivation.report.operations["create_message"];
    assert_eq!(outcome.status, DerivationStatus::Derived);

    let request = &derivation.definition.models["CreateMessagesRequest"];
    assert_eq!(
        request.constructor.as_deref(),
        Some(&["required_value".to_owned(), "required_nullable".to_owned()][..])
    );

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("generate");

    let types = &generated.files["facade_types.rs"];
    assert!(types.contains(
        "pub fn new(required_value: impl Into<String>, required_nullable: Option<String>)"
    ));
    assert!(types.contains("required_nullable,"));
    assert!(types.contains("pub fn optional_value(mut self, optional_value: impl Into<String>)"));
    assert!(
        types.contains(
            "pub fn optional_nullable(mut self, optional_nullable: impl Into<String>)"
        )
    );
    assert!(types.contains("pub fn optional_nullable_null(mut self)"));
}

#[test]
fn missing_presence_evidence_fails_closed() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .structs
        .get_mut("OpaqueRequest")
        .expect("request binding")[1]
        .serialized_presence = None;

    assert!(
        derive(DeriveInput {
            openapi,
            bindings,
            surface,
            overrides: SdkOverrides::default(),
        })
        .is_err()
    );
}

#[test]
fn mismatched_or_unproven_presence_semantics_reject_projection() {
    let (openapi, bindings, surface) = fixture();
    let cases = [
        (0usize, SerializedPresenceBinding::Never),
        (1, SerializedPresenceBinding::OmitIfNone),
        (2, SerializedPresenceBinding::Always),
        (3, SerializedPresenceBinding::Conditional),
    ];

    for (index, presence) in cases {
        let mut candidate = bindings.clone();
        candidate.structs.get_mut("OpaqueRequest").expect("request")[index].serialized_presence =
            Some(presence);
        let result = derive_fixture(openapi.clone(), candidate, surface.clone());
        let outcome = &result.report.operations["create_message"];
        assert_eq!(outcome.status, DerivationStatus::Rejected);
        assert_eq!(outcome.reason.code, "bindings.no_structural_match");
    }
}
