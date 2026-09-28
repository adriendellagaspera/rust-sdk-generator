use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

#[test]
fn projects_optional_query_scalar_enum_without_exposing_backend_symbol() {
    let openapi = OpenApi(serde_json::json!({
        "openapi": "3.1.0",
        "info": {"title": "Parameter enum fixture", "version": "1"},
        "paths": {
            "/audio/voices": {
                "get": {
                    "operationId": "list_voices",
                    "parameters": [{
                        "name": "type",
                        "in": "query",
                        "required": false,
                        "schema": {
                            "type": "string",
                            "enum": ["stock", "custom"]
                        }
                    }],
                    "responses": {
                        "204": {"description": "Done"}
                    }
                }
            }
        }
    }));
    let bindings: Bindings = serde_json::from_value(serde_json::json!({
        "schema_version": 3,
        "structs": {},
        "enums": {
            "OpaqueVoiceType": [
                {"name": "Stock", "payload": null, "wire_name": "stock"},
                {"name": "Custom", "payload": null, "wire_name": "custom"}
            ]
        },
        "aliases": {},
        "operations": {
            "opaque_list_voices": {
                "name": "opaque_list_voices",
                "parameters": [{
                    "name": "r#type",
                    "type": "Option<OpaqueVoiceType>"
                }],
                "return_type": "Result<(), Error>",
                "success_type": "()",
                "stream": null,
                "metadata": {
                    "kind": "call_shape",
                    "source_operation": {
                        "operation_id": "list_voices",
                        "method": "GET",
                        "path": "/audio/voices"
                    },
                    "emitted_operation_id": "list_voices",
                    "representation": {"kind": "empty"},
                    "success_statuses": ["204"],
                    "request_discriminators": [],
                    "parameter_wires": [{
                        "rust_name": "r#type",
                        "location": "query",
                        "wire_name": "type"
                    }],
                    "stream_abi": null
                }
            }
        },
        "symbol_paths": {
            "OpaqueVoiceType": "crate::generated::client::OpaqueVoiceType"
        },
        "binding": {
            "client": {
                "type_path": "crate::generated::client::HttpClient",
                "constructor": "new",
                "api_key_builder": "with_api_key",
                "base_url_builder": "with_base_url"
            },
            "type_preludes": ["crate::generated::client::*"]
        }
    }))
    .expect("bindings");

    let mut surface = PublicSdkSurface::default();
    surface
        .operations
        .insert("list_voices".into(), vec!["audio_voices.list".into()]);

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive parameter enum");
    let outcome = &derivation.report.operations["list_voices"];
    assert_eq!(outcome.status, DerivationStatus::Derived);

    let operation = &derivation.definition.resources["audio_voices"].operations["list"];
    let adapter = &operation.parameter_adapters["r#type"];
    assert_eq!(adapter.model, "ListAudioVoicesRequestType");
    assert_eq!(adapter.location, "query");
    assert_eq!(adapter.wire_name, "type");

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("generate parameter enum");

    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("pub enum ListAudioVoicesRequestType"));
    assert!(
        types.contains("impl __RustSdkIntoRaw<OpaqueVoiceType> for ListAudioVoicesRequestType")
    );

    let resource = &generated.files["audio_voices.rs"];
    assert!(resource.contains("r#type: Option<ListAudioVoicesRequestType>"));
    assert!(resource.contains("r#type.map(__RustSdkIntoRaw::into_raw)"));
    assert!(!resource.contains("Option<crate::generated::client::OpaqueVoiceType>"));
}
