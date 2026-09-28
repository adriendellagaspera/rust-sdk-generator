use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

#[test]
fn projects_optional_scalar_enum_wire_parameter_without_backend_leak() {
    let openapi = OpenApi(serde_json::json!({
        "openapi": "3.1.0",
        "info": {"title": "Parameter enum fixture", "version": "1"},
        "paths": {
            "/voices": {
                "get": {
                    "operationId": "list_voices",
                    "parameters": [
                        {
                            "name": "kind",
                            "in": "query",
                            "required": false,
                            "schema": {
                                "type": "string",
                                "enum": ["preset", "generated"]
                            }
                        }
                    ],
                    "responses": {
                        "204": {"description": "No content"}
                    }
                }
            }
        }
    }));

    let bindings: Bindings = serde_json::from_value(serde_json::json!({
        "schema_version": 2,
        "structs": {},
        "enums": {
            "OpaqueVoiceKind": [
                {"name": "Preset", "payload": null, "wire_name": "preset"},
                {"name": "Generated", "payload": null, "wire_name": "generated"}
            ]
        },
        "aliases": {},
        "operations": {
            "opaque_list": {
                "name": "opaque_list",
                "parameters": [
                    {"name": "kind", "type": "Option<OpaqueVoiceKind>"}
                ],
                "return_type": "Result<(), Error>",
                "success_type": "()"
            }
        },
        "symbol_paths": {
            "OpaqueVoiceKind": "crate::generated::client::OpaqueVoiceKind"
        },
        "binding": {
            "client": {
                "type_path": "crate::generated::client::Client",
                "constructor": "new",
                "api_key_builder": "with_api_key",
                "base_url_builder": "with_base_url"
            },
            "type_preludes": ["crate::generated::client::*"]
        }
    }))
    .expect("bindings");

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface: PublicSdkSurface::default(),
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    assert_eq!(
        derivation.report.operations["list_voices"].status,
        DerivationStatus::Derived
    );

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("generate closed parameter enum facade");

    let rendered = generated
        .files
        .values()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("pub enum"));
    assert!(rendered.contains("Preset"));
    assert!(rendered.contains("Generated"));
    assert!(rendered.contains(".map(__RustSdkIntoRaw::into_raw)"));
    assert!(!rendered.contains("Option<crate::generated::client::OpaqueVoiceKind>"));
}
