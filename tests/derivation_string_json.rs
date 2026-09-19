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
    .expect("fixture surface");

    openapi.0["paths"]["/reports/title"] = serde_json::json!({
        "get": {
            "operationId": "read_title",
            "responses": {
                "200": {"description": "JSON string", "content": {
                    "application/json": {"schema": {"type": "string", "title": "Report title"}}
                }}
            }
        }
    });
    let mut raw = bindings.operations["raw_read_report"].clone();
    raw.name = "raw_read_title".into();
    raw.success_type = "ReportTitle".into();
    raw.return_type = "Result<ReportTitle, Error>".into();
    let metadata = raw.metadata.as_mut().expect("canonical metadata");
    metadata.source_operation.operation_id = "read_title".into();
    metadata.source_operation.path = "/reports/title".into();
    metadata.emitted_operation_id = "read_title".into();
    metadata.representation = rust_sdk_generator::ResponseRepresentationBinding::Json {
        schema_name: "ReportTitle".into(),
        media_type: "application/json".into(),
    };
    metadata.success_statuses = vec!["200".into()];
    bindings.operations.insert("raw_read_title".into(), raw);
    bindings
        .aliases
        .insert("ReportTitle".into(), "String".into());
    bindings.symbol_paths.insert(
        "ReportTitle".into(),
        "crate::generated::types::ReportTitle".into(),
    );
    surface
        .operations
        .insert("read_title".into(), vec!["reports.title".into()]);
    (openapi, bindings, surface)
}

#[test]
fn derives_and_generates_a_proven_string_json_alias() {
    let (openapi, bindings, surface) = fixture();
    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("string response derivation");
    assert_eq!(
        derivation.report.operations["read_title"].status,
        DerivationStatus::Derived
    );
    let model = &derivation.definition.models["TitleReportsResponse"];
    assert_eq!(model.raw.as_deref(), Some("ReportTitle"));
    assert_eq!(model.type_alias, Some(true));
    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("string response generation");
    assert!(
        generated
            .inventory
            .models
            .contains(&"TitleReportsResponse".to_owned())
    );
}

#[test]
fn rejects_binary_enum_and_unbound_string_json_aliases() {
    let (openapi, bindings, surface) = fixture();
    for schema in [
        serde_json::json!({"type": "string", "format": "binary"}),
        serde_json::json!({"type": "string", "enum": ["limited"]}),
        serde_json::json!({"type": "object"}),
    ] {
        let mut drifted = openapi.clone();
        drifted.0["paths"]["/reports/title"]["get"]["responses"]["200"]["content"]["application/json"]
            ["schema"] = schema;
        let derivation = derive(DeriveInput {
            openapi: drifted,
            bindings: bindings.clone(),
            surface: surface.clone(),
            overrides: SdkOverrides::default(),
        })
        .expect("closed-world derivation");
        assert_eq!(
            derivation.report.operations["read_title"].status,
            DerivationStatus::Rejected
        );
    }
    let mut unbound = bindings;
    unbound
        .aliases
        .insert("ReportTitle".into(), "serde_json::Value".into());
    let derivation = derive(DeriveInput {
        openapi,
        bindings: unbound,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("closed-world derivation");
    assert_eq!(
        derivation.report.operations["read_title"].status,
        DerivationStatus::Rejected
    );
}
