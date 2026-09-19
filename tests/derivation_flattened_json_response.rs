use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let mut openapi: OpenApi = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-response/openapi.json"
    ))
    .expect("OpenAPI");
    let mut bindings: Bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-response/rust-bindings.json"
    ))
    .expect("bindings");
    let mut surface: PublicSdkSurface = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-response/surface.json"
    ))
    .expect("surface");
    openapi.0["components"]["schemas"]["ToolResult"] = serde_json::json!({
        "type": "object",
        "properties": {
            "content": {"type": "array", "items": {"type": "string"}},
            "metadata": {"anyOf": [{"type": "string"}, {"type": "null"}]}
        },
        "required": ["content"],
        "additionalProperties": true
    });
    openapi.0["paths"]["/tool/result"] = serde_json::json!({
        "get": {"operationId": "read_tool_result", "responses": {
            "200": {"description": "result", "content": {
                "application/json": {"schema": {"$ref": "#/components/schemas/ToolResult"}}
            }}
        }}
    });
    let mut raw = bindings.operations["raw_read_report"].clone();
    raw.name = "raw_read_tool_result".into();
    raw.success_type = "ToolResult".into();
    raw.return_type = "Result<ToolResult, Error>".into();
    let meta = raw.metadata.as_mut().expect("canonical metadata");
    meta.source_operation.operation_id = "read_tool_result".into();
    meta.source_operation.path = "/tool/result".into();
    meta.emitted_operation_id = "read_tool_result".into();
    meta.representation = rust_sdk_generator::ResponseRepresentationBinding::Json {
        schema_name: "ToolResult".into(),
        media_type: "application/json".into(),
    };
    meta.success_statuses = vec!["200".into()];
    bindings.operations.insert("raw_read_tool_result".into(), raw);
    bindings.structs.insert(
        "ToolResult".into(),
        serde_json::from_value(serde_json::json!([
            {"name": "content", "wire_name": "content", "type": "Vec<String>"},
            {"name": "metadata", "wire_name": "metadata", "type": "Option<Option<String>>"},
            {"name": "additional_properties", "wire_name": null,
              "type": "std::collections::BTreeMap<String, serde_json::Value>"}
        ]))
        .expect("flat raw response"),
    );
    bindings.symbol_paths.insert(
        "ToolResult".into(),
        "crate::generated::types::ToolResult".into(),
    );
    surface.operations.insert(
        "read_tool_result".into(),
        vec!["tools.read_result".into()],
    );
    (openapi, bindings, surface)
}

#[test]
fn derives_and_generates_exact_flattened_response() {
    let (openapi, bindings, surface) = fixture();
    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    assert_eq!(
        derivation.report.operations["read_tool_result"].status,
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
fn rejects_unproven_extra_fields_and_map_types() {
    let (openapi, bindings, surface) = fixture();
    for extra in [
        serde_json::json!(false),
        serde_json::json!({"type": "string"}),
    ] {
        let mut drifted = openapi.clone();
        drifted.0["components"]["schemas"]["ToolResult"]["additionalProperties"] = extra;
        let derivation = derive(DeriveInput {
            openapi: drifted,
            bindings: bindings.clone(),
            surface: surface.clone(),
            overrides: SdkOverrides::default(),
        })
        .expect("derive");
        assert_eq!(
            derivation.report.operations["read_tool_result"].status,
            DerivationStatus::Rejected
        );
    }
    let mut raw = bindings;
    raw.structs.get_mut("ToolResult").expect("response")[2].type_name =
        "std::collections::BTreeMap<String, String>".into();
    let derivation = derive(DeriveInput {
        openapi,
        bindings: raw,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    assert_eq!(
        derivation.report.operations["read_tool_result"].status,
        DerivationStatus::Rejected
    );
}
