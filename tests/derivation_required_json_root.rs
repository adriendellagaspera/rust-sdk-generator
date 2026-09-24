use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_value(serde_json::json!({
        "openapi": "3.1.0",
        "info": {"title": "Required JSON root fixture", "version": "1"},
        "paths": {
            "/members": {"post": {
                "operationId": "create_members",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "type": "array",
                        "items": {"$ref": "#/components/schemas/Member"}
                    }}}
                },
                "responses": {"204": {"description": "done"}}
            }},
            "/metrics": {"put": {
                "operationId": "update_metrics",
                "requestBody": {
                    "required": true,
                    "content": {"application/json": {"schema": {
                        "anyOf": [
                            {"$ref": "#/components/schemas/Online"},
                            {"$ref": "#/components/schemas/Offline"}
                        ]
                    }}}
                },
                "responses": {"204": {"description": "done"}}
            }}
        },
        "components": {"schemas": {
            "Member": {
                "type": "object",
                "required": ["name"],
                "properties": {"name": {"type": "string"}}
            },
            "Online": {
                "type": "object",
                "required": ["online"],
                "properties": {"online": {"type": "boolean"}}
            },
            "Offline": {
                "type": "object",
                "required": ["batch"],
                "properties": {"batch": {"type": "string"}}
            }
        }}
    }))
    .expect("OpenAPI");

    let bindings = serde_json::from_value(serde_json::json!({
        "schema_version": 3,
        "structs": {
            "Member": [{"name": "name", "wire_name": "name", "type": "String"}],
            "Online": [{"name": "online", "wire_name": "online", "type": "bool"}],
            "Offline": [{"name": "batch", "wire_name": "batch", "type": "String"}]
        },
        "enums": {
            "MetricsRequest": [
                {"name": "Online", "payload": "Online", "wire_name": "online"},
                {"name": "Offline", "payload": "Offline", "wire_name": "offline"}
            ]
        },
        "aliases": {"MembersRequest": "Vec<Member>"},
        "operations": {
            "raw_create_members": {
                "name": "raw_create_members",
                "parameters": [{"name": "request", "type": "MembersRequest"}],
                "return_type": "Result<(), Error>",
                "success_type": "()",
                "stream": null,
                "metadata": {
                    "kind": "call_shape",
                    "source_operation": {
                        "operation_id": "create_members",
                        "method": "POST",
                        "path": "/members"
                    },
                    "emitted_operation_id": "create_members",
                    "representation": {"kind": "empty"},
                    "success_statuses": ["204"],
                    "request_discriminators": [],
                    "stream_abi": null
                }
            },
            "raw_update_metrics": {
                "name": "raw_update_metrics",
                "parameters": [{"name": "request", "type": "MetricsRequest"}],
                "return_type": "Result<(), Error>",
                "success_type": "()",
                "stream": null,
                "metadata": {
                    "kind": "call_shape",
                    "source_operation": {
                        "operation_id": "update_metrics",
                        "method": "PUT",
                        "path": "/metrics"
                    },
                    "emitted_operation_id": "update_metrics",
                    "representation": {"kind": "empty"},
                    "success_statuses": ["204"],
                    "request_discriminators": [],
                    "stream_abi": null
                }
            }
        },
        "symbol_paths": {
            "MembersRequest": "crate::raw::MembersRequest",
            "Member": "crate::raw::Member",
            "MetricsRequest": "crate::raw::MetricsRequest",
            "Online": "crate::raw::Online",
            "Offline": "crate::raw::Offline"
        },
        "binding": {
            "client": {
                "type_path": "crate::raw::Client",
                "constructor": "new",
                "api_key_builder": "with_api_key",
                "base_url_builder": "with_base_url"
            },
            "type_preludes": ["crate::raw::*"]
        }
    }))
    .expect("bindings");

    let surface = serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "client": "RootRequestClient",
        "operations": {
            "create_members": ["admin.users.create_many"],
            "update_metrics": ["rag.metrics.update"]
        }
    }))
    .expect("surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_required_array_and_union_root_requests_as_owned_raw_views() {
    let (openapi, bindings, surface) = fixture();
    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    for operation_id in ["create_members", "update_metrics"] {
        let outcome = &derivation.report.operations[operation_id];
        assert_eq!(outcome.status, DerivationStatus::Derived, "{operation_id}");
        assert_eq!(outcome.reason.code, "inference.structurally_proven");
    }
    assert!(derivation.definition.models.values().any(|model| {
        model.raw.as_deref() == Some("MembersRequest")
            && model.constructor.is_none()
            && model
                .accessors
                .as_ref()
                .is_some_and(indexmap::IndexMap::is_empty)
    }));
    assert!(derivation.definition.models.values().any(|model| {
        model.raw.as_deref() == Some("MetricsRequest")
            && model.constructor.is_none()
            && model
                .accessors
                .as_ref()
                .is_some_and(indexmap::IndexMap::is_empty)
    }));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("generate");
    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("MembersRequest"));
    assert!(types.contains("MetricsRequest"));
    assert!(types.contains("into_raw"));
}

#[test]
fn rejects_root_shape_drift_before_generation() {
    let (mut openapi, bindings, surface) = fixture();
    *openapi
        .0
        .pointer_mut("/paths/~1members/post/requestBody/content/application~1json/schema/type")
        .expect("array type") = serde_json::json!("object");

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    assert_eq!(
        derivation.report.operations["create_members"].status,
        DerivationStatus::Rejected
    );
}
