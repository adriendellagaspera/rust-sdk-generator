use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-response/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-response/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-response/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_and_generates_canonical_json_and_empty_representations() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let read = &derivation.report.operations["read_report"];
    assert_eq!(read.status, DerivationStatus::Derived);
    assert_eq!(read.reason.code, "inference.structurally_proven");
    assert_eq!(read.binding.as_deref(), Some("raw_read_report"));

    let purge = &derivation.report.operations["purge_reports"];
    assert_eq!(purge.status, DerivationStatus::Derived);
    assert_eq!(purge.reason.code, "inference.structurally_proven");
    assert_eq!(purge.binding.as_deref(), Some("raw_purge_reports"));

    let read_operation = &derivation.definition.resources["reports"].operations["current"];
    assert_eq!(read_operation.raw_method.as_deref(), Some("raw_read_report"));
    assert_eq!(
        read_operation.response.as_deref(),
        Some("CurrentReportsResponse")
    );
    assert_eq!(read_operation.empty_response, None);

    let purge_operation = &derivation.definition.resources["reports"].operations["purge"];
    assert_eq!(
        purge_operation.raw_method.as_deref(),
        Some("raw_purge_reports")
    );
    assert_eq!(purge_operation.response, None);
    assert_eq!(purge_operation.empty_response, Some(true));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    assert_eq!(generated.inventory.client, "ReportsClient");
    assert!(
        generated
            .inventory
            .models
            .contains(&"CurrentReportsResponse".to_owned())
    );
    assert!(generated.files.values().any(|source| {
        source.contains("self.raw.raw_read_report(")
    }));
    assert!(generated.files.values().any(|source| {
        source.contains("self.raw.raw_purge_reports(")
    }));
}

#[test]
fn lowering_revalidates_selected_json_statuses() {
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
            "/paths/~1reports~1current/get/responses/206/content/application~1json/schema",
        )
        .expect("206 JSON schema") = serde_json::json!({
        "type": "object",
        "properties": {
            "different": {"type": "string"}
        }
    });

    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect_err("selected response drift must fail lowering");

    assert_eq!(
        error.diagnostic.code,
        "lower.response_representation_drift"
    );
}

#[test]
fn lowering_revalidates_selected_empty_statuses() {
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
        .pointer_mut("/paths/~1reports~1cache/delete/responses/202")
        .and_then(serde_json::Value::as_object_mut)
        .expect("202 response")
        .insert(
            "content".into(),
            serde_json::json!({
                "application/json": {"schema": {"$ref": "#/components/schemas/Report"}}
            }),
        );

    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect_err("selected empty response drift must fail lowering");

    assert_eq!(error.diagnostic.code, "lower.empty_response_drift");
}
