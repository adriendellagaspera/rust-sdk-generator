use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

#[test]
fn projects_optional_structured_query_parameter_without_backend_leak() {
    let openapi = OpenApi(serde_json::json!({
        "openapi": "3.1.0",
        "info": {"title": "Structured parameter fixture", "version": "1"},
        "components": {
            "schemas": {
                "Filters": {
                    "type": "object",
                    "properties": {
                        "active": {
                            "anyOf": [
                                {"type": "boolean"},
                                {"type": "null"}
                            ]
                        }
                    }
                }
            }
        },
        "paths": {
            "/widgets": {
                "get": {
                    "operationId": "list_widgets",
                    "parameters": [
                        {
                            "name": "query_filters",
                            "in": "query",
                            "required": false,
                            "schema": {
                                "$ref": "#/components/schemas/Filters"
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
        "schema_version": 5,
        "structs": {
            "OpaqueFilters": [
                {
                    "name": "active",
                    "wire_name": "active",
                    "type": "Option<Option<bool>>",
                    "serialized_presence": "omit_if_none"
                }
            ]
        },
        "enums": {},
        "aliases": {},
        "operations": {
            "opaque_list": {
                "name": "opaque_list",
                "parameters": [
                    {"name": "query_filters", "type": "Option<OpaqueFilters>"}
                ],
                "return_type": "Result<(), Error>",
                "success_type": "()",
                "metadata": {
                    "kind": "call_shape",
                    "source_operation": {
                        "operation_id": "list_widgets",
                        "method": "GET",
                        "path": "/widgets"
                    },
                    "emitted_operation_id": "list_widgets",
                    "representation": {"kind": "empty"},
                    "success_statuses": ["204"],
                    "request_discriminators": [],
                    "parameter_wires": [
                        {
                            "rust_name": "query_filters",
                            "location": "query",
                            "wire_name": "query_filters"
                        }
                    ],
                    "stream_abi": null
                }
            }
        },
        "symbol_paths": {
            "OpaqueFilters": "crate::generated::types::OpaqueFilters"
        },
        "binding": {
            "client": {
                "type_path": "crate::generated::client::Client",
                "constructor": "new",
                "api_key_builder": "with_api_key",
                "base_url_builder": "with_base_url"
            },
            "type_preludes": [
                "crate::generated::types::*",
                "crate::generated::client::*"
            ]
        }
    }))
    .expect("bindings");

    let mut surface = PublicSdkSurface::default();
    surface
        .operations
        .insert("list_widgets".into(), vec!["widgets.list".into()]);

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    assert_eq!(
        derivation.report.operations["list_widgets"].status,
        DerivationStatus::Derived
    );

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("generate closed structured parameter facade");

    let rendered = generated
        .files
        .values()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("pub struct ListWidgetsRequestQueryFilters"));
    assert!(rendered.contains("pub fn active(mut self, active: bool) -> Self"));
    assert!(rendered.contains("pub fn active_null(mut self) -> Self"));
    assert!(rendered.contains("query_filters: Option<ListWidgetsRequestQueryFilters>"));
    assert!(
        rendered
            .contains("impl __RustSdkIntoRaw<crate::generated::types::OpaqueFilters> for ListWidgetsRequestQueryFilters")
    );
    assert!(rendered.contains("request.query_filters.map(__RustSdkIntoRaw::into_raw)"));
    assert!(!rendered.contains("Option<crate::generated::types::OpaqueFilters>"));
}
