use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture(raw_request_type: &str) -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_value(serde_json::json!({
        "openapi": "3.1.0",
        "info": {"title": "Optional nullable root fixture", "version": "1"},
        "paths": {
            "/schedules/{schedule_id}/pause": {"post": {
                "operationId": "pause_schedule",
                "parameters": [{
                    "name": "schedule_id",
                    "in": "path",
                    "required": true,
                    "schema": {"type": "string"}
                }],
                "requestBody": {
                    "content": {"application/json": {"schema": {
                        "anyOf": [
                            {"$ref": "#/components/schemas/PauseRequest"},
                            {"type": "null"}
                        ],
                        "title": "Schedule state"
                    }}}
                },
                "responses": {"204": {"description": "done"}}
            }}
        },
        "components": {"schemas": {
            "PauseRequest": {
                "type": "object",
                "properties": {
                    "note": {
                        "anyOf": [
                            {"type": "string"},
                            {"type": "null"}
                        ]
                    }
                }
            }
        }}
    }))
    .expect("OpenAPI");

    let bindings = serde_json::from_value(serde_json::json!({
        "schema_version": 3,
        "structs": {
            "PauseRequest": [{
                "name": "note",
                "wire_name": "note",
                "type": "Option<Option<String>>"
            }]
        },
        "enums": {},
        "aliases": {"PauseAlias": "PauseRequest"},
        "operations": {
            "raw_pause_schedule": {
                "name": "raw_pause_schedule",
                "parameters": [
                    {"name": "schedule_id", "type": "impl AsRef<str>"},
                    {"name": "request", "type": raw_request_type}
                ],
                "return_type": "Result<(), Error>",
                "success_type": "()",
                "stream": null,
                "metadata": {
                    "kind": "call_shape",
                    "source_operation": {
                        "operation_id": "pause_schedule",
                        "method": "POST",
                        "path": "/schedules/{schedule_id}/pause"
                    },
                    "emitted_operation_id": "pause_schedule",
                    "representation": {"kind": "empty"},
                    "success_statuses": ["204"],
                    "request_discriminators": [],
                    "stream_abi": null
                }
            }
        },
        "symbol_paths": {
            "PauseRequest": "crate::raw::PauseRequest",
            "PauseAlias": "crate::raw::PauseAlias"
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
        "client": "ScheduleClient",
        "operations": {
            "pause_schedule": ["workflows.schedules.pause"]
        }
    }))
    .expect("surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_exact_tri_state_root_and_preserves_public_absent_null_value() {
    let (openapi, bindings, surface) = fixture("Option<Option<PauseAlias>>");
    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["pause_schedule"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");

    let request = derivation
        .definition
        .models
        .values()
        .find(|model| model.raw.as_deref() == Some("PauseAlias"))
        .expect("owned inner request view");
    assert!(request.constructor.is_none());
    assert!(
        request
            .accessors
            .as_ref()
            .is_some_and(indexmap::IndexMap::is_empty)
    );

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("generate");
    let client = &generated.files["client.rs"];
    assert!(
        client.contains("request: Option<Option<PauseWorkflowsSchedulesRequest>>"),
        "{client}"
    );
    assert!(
        client.contains("request.map(|request| request.map(|request| request.into_raw()))"),
        "{client}"
    );
}

#[test]
fn rejects_shallow_option_and_constrained_nullable_root() {
    let (openapi, bindings, surface) = fixture("Option<PauseAlias>");
    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    assert_eq!(
        derivation.report.operations["pause_schedule"].status,
        DerivationStatus::Rejected
    );

    let (mut openapi, bindings, surface) = fixture("Option<Option<PauseAlias>>");
    let schema = openapi
        .0
        .pointer_mut(
            "/paths/~1schedules~1{schedule_id}~1pause/post/requestBody/content/application~1json/schema",
        )
        .expect("request schema")
        .as_object_mut()
        .expect("request schema object");
    schema.insert("minProperties".into(), serde_json::json!(1));

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    assert_eq!(
        derivation.report.operations["pause_schedule"].status,
        DerivationStatus::Rejected
    );
}
