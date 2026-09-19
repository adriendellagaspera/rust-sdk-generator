use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let mut openapi: OpenApi = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-response/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let mut bindings: Bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-response/rust-bindings.json"
    ))
    .expect("fixture Bindings");
    let mut surface: PublicSdkSurface = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-response/surface.json"
    ))
    .expect("fixture public surface");

    openapi.0["paths"]["/reports/opaque"] = serde_json::json!({
        "get": {
            "operationId": "read_opaque",
            "responses": {
                "200": {"description": "arbitrary JSON", "content": {
                    "application/json": {"schema": {}}
                }}
            }
        }
    });
    let mut raw = bindings.operations["raw_read_report"].clone();
    raw.name = "raw_read_opaque".into();
    raw.success_type = "OpaqueReport".into();
    raw.return_type = "Result<OpaqueReport, Error>".into();
    let metadata = raw.metadata.as_mut().expect("canonical metadata");
    metadata.source_operation.operation_id = "read_opaque".into();
    metadata.source_operation.path = "/reports/opaque".into();
    metadata.emitted_operation_id = "read_opaque".into();
    metadata.representation = rust_sdk_generator::ResponseRepresentationBinding::Json {
        schema_name: "OpaqueReport".into(),
        media_type: "application/json".into(),
    };
    metadata.success_statuses = vec!["200".into()];
    bindings.operations.insert("raw_read_opaque".into(), raw);
    bindings
        .aliases
        .insert("OpaqueReport".into(), "serde_json::Value".into());
    bindings.symbol_paths.insert(
        "OpaqueReport".into(),
        "crate::generated::types::OpaqueReport".into(),
    );
    surface
        .operations
        .insert("read_opaque".into(), vec!["reports.opaque".into()]);
    (openapi, bindings, surface)
}

#[test]
fn derives_and_generates_only_a_proven_unconstrained_json_alias() {
    let (openapi, bindings, surface) = fixture();
    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("unconstrained JSON alias derivation");

    assert_eq!(
        derivation.report.operations["read_opaque"].status,
        DerivationStatus::Derived
    );
    let model = &derivation.definition.models["OpaqueReportsResponse"];
    assert_eq!(model.raw.as_deref(), Some("OpaqueReport"));
    assert_eq!(model.type_alias, Some(true));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("unconstrained JSON alias generation");
    assert!(
        generated
            .inventory
            .models
            .contains(&"OpaqueReportsResponse".to_owned())
    );
}

#[test]
fn rejects_typed_or_unbound_json_schema_instead_of_guessing() {
    let (mut openapi, bindings, surface) = fixture();
    openapi.0["paths"]["/reports/opaque"]["get"]["responses"]["200"]["content"]
        ["application/json"]["schema"] = serde_json::json!({"type": "string"});
    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("closed-world derivation");
    assert_eq!(
        derivation.report.operations["read_opaque"].status,
        DerivationStatus::Rejected
    );
}
