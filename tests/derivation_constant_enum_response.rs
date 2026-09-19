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

    openapi.0["paths"]["/reports/archive"] = serde_json::json!({
        "post": {
            "operationId": "archive_report",
            "responses": {"200": {"description": "archived report", "content": {
                "application/json": {"schema": {"$ref": "#/components/schemas/ArchivedReport"}}
            }}}
        }
    });
    openapi.0["components"]["schemas"]["ArchivedReport"] = serde_json::json!({
        "type": "object",
        "properties": {
            "id": {"type": "string"},
            "archived": {"type": "boolean", "default": true},
            "tag": {"type": "string", "const": "archive", "default": "archive"}
        },
        "required": ["id"]
    });
    let mut raw = bindings.operations["raw_read_report"].clone();
    raw.name = "raw_archive_report".into();
    raw.success_type = "ArchivedReport".into();
    raw.return_type = "Result<ArchivedReport, Error>".into();
    let metadata = raw.metadata.as_mut().expect("canonical metadata");
    metadata.source_operation.operation_id = "archive_report".into();
    metadata.source_operation.method = "POST".into();
    metadata.source_operation.path = "/reports/archive".into();
    metadata.emitted_operation_id = "archive_report".into();
    metadata.representation = rust_sdk_generator::ResponseRepresentationBinding::Json {
        schema_name: "ArchivedReport".into(),
        media_type: "application/json".into(),
    };
    metadata.success_statuses = vec!["200".into()];
    bindings.operations.insert("raw_archive_report".into(), raw);
    bindings.structs.insert(
        "ArchivedReport".into(),
        serde_json::from_value(serde_json::json!([
            {"name": "archived", "wire_name": "archived", "type": "Option<bool>"},
            {"name": "id", "wire_name": "id", "type": "String"},
            {"name": "tag", "wire_name": "tag", "type": "Option<ArchiveTag>"}
        ]))
        .expect("raw response fields"),
    );
    bindings.enums.insert(
        "ArchiveTag".into(),
        serde_json::from_value(serde_json::json!([
            {"name": "Archive", "wire_name": "archive", "payload": null}
        ]))
        .expect("raw constant enum"),
    );
    bindings.symbol_paths.insert(
        "ArchivedReport".into(),
        "crate::generated::types::ArchivedReport".into(),
    );
    bindings.symbol_paths.insert(
        "ArchiveTag".into(),
        "crate::generated::types::ArchiveTag".into(),
    );
    surface
        .operations
        .insert("archive_report".into(), vec!["reports.archive".into()]);
    (openapi, bindings, surface)
}

#[test]
fn derives_and_generates_exact_constant_enum_response() {
    let (openapi, bindings, surface) = fixture();
    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    assert_eq!(
        derivation.report.operations["archive_report"].status,
        DerivationStatus::Derived
    );
    generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("generate");
}

#[test]
fn rejects_constant_value_and_optional_depth_drift() {
    let (openapi, bindings, surface) = fixture();
    for schema in [
        serde_json::json!({"type": "string", "const": "other"}),
        serde_json::json!({"type": "integer", "const": 1}),
    ] {
        let mut drifted = openapi.clone();
        drifted.0["components"]["schemas"]["ArchivedReport"]["properties"]["tag"] = schema;
        let derivation = derive(DeriveInput {
            openapi: drifted,
            bindings: bindings.clone(),
            surface: surface.clone(),
            overrides: SdkOverrides::default(),
        })
        .expect("closed world");
        assert_eq!(
            derivation.report.operations["archive_report"].status,
            DerivationStatus::Rejected
        );
    }

    let mut raw = bindings.clone();
    raw.structs
        .get_mut("ArchivedReport")
        .expect("response struct")[2]
        .type_name = "Option<Option<ArchiveTag>>".into();
    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: raw,
        surface: surface.clone(),
        overrides: SdkOverrides::default(),
    })
    .expect("closed world");
    assert_eq!(
        derivation.report.operations["archive_report"].status,
        DerivationStatus::Rejected
    );

}
