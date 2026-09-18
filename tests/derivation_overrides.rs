use std::collections::BTreeMap;

use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, OperationOverride,
    PublicSdkSurface, Runtime, SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let mut openapi: OpenApi =
        serde_json::from_str(include_str!("fixtures/derivation-update/openapi.json"))
            .expect("fixture OpenAPI");
    openapi
        .0
        .pointer_mut("/components/schemas/UpdateJobRequest/properties")
        .and_then(serde_json::Value::as_object_mut)
        .expect("request properties")
        .insert("archived".into(), serde_json::json!({"type": "boolean"}));

    let mut bindings: Bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-update/rust-bindings.json"
    ))
    .expect("fixture bindings");
    bindings
        .structs
        .get_mut("UpdateJobRequest")
        .expect("request binding")
        .push(rust_sdk_generator::FieldBinding {
            name: "archived".into(),
            type_name: "Option<bool>".into(),
        });

    let surface: PublicSdkSurface =
        serde_json::from_str(include_str!("fixtures/derivation-update/surface.json"))
            .expect("fixture surface");
    (openapi, bindings, surface)
}

fn request_override(field: &str, value: Option<bool>) -> SdkOverrides {
    let mut overrides = SdkOverrides::default();
    overrides.operations.insert(
        "revise_job".into(),
        OperationOverride {
            request_overrides: BTreeMap::from([(field.into(), value)]),
        },
    );
    overrides
}

#[test]
fn applies_request_override_after_generic_inference_and_generates_it() {
    let (openapi, bindings, surface) = fixture();
    let overrides = request_override("archived", Some(true));

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides,
    })
    .expect("derive");

    let outcome = &derivation.report.operations["revise_job"];
    assert_eq!(outcome.status, DerivationStatus::Overridden);
    assert_eq!(outcome.reason.code, "override.request_overrides");
    assert_eq!(outcome.reason.detail.as_deref(), Some("archived"));
    assert_eq!(outcome.binding.as_deref(), Some("call_42"));

    let operation = &derivation.definition.resources["work_jobs"].operations["update"];
    assert_eq!(
        operation
            .request_overrides
            .as_ref()
            .expect("request overrides")["archived"],
        Some(true)
    );

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    assert!(generated.files.values().any(|source| {
        source.contains("raw.archived = Some(true);")
    }));
}

#[test]
fn null_request_override_is_explicitly_emitted() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: request_override("archived", None),
    })
    .expect("derive");

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    assert!(generated.files.values().any(|source| {
        source.contains("raw.archived = None;")
    }));
}

#[test]
fn rejects_request_override_without_optional_boolean_proof() {
    let (openapi, bindings, surface) = fixture();

    let error = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: request_override("priority", Some(false)),
    })
    .expect_err("non-Boolean request override must fail");

    assert_eq!(
        error.diagnostic.code,
        "overrides.invalid_request_override"
    );
    assert_eq!(
        error.diagnostic.path.as_deref(),
        Some("overrides.operations.revise_job.request_overrides.priority")
    );
}

#[test]
fn rejects_exclusion_override_conflict() {
    let (openapi, bindings, surface) = fixture();
    let mut overrides = request_override("archived", Some(false));
    overrides
        .excluded_operations
        .insert("revise_job".into(), "consumer excludes operation".into());

    let error = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides,
    })
    .expect_err("exclude + override must fail");

    assert_eq!(error.diagnostic.code, "overrides.conflict");
    assert_eq!(
        error.diagnostic.path.as_deref(),
        Some("overrides.operations.revise_job")
    );
}

#[test]
fn rejects_override_when_generic_derivation_did_not_succeed() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .operations
        .get_mut("call_42")
        .expect("operation binding")
        .success_type = "String".into();

    let error = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: request_override("archived", Some(true)),
    })
    .expect_err("override must not rescue failed generic inference");

    assert_eq!(error.diagnostic.code, "overrides.unapplied");
    assert_eq!(
        error.diagnostic.path.as_deref(),
        Some("overrides.operations.revise_job")
    );
}

#[test]
fn operation_override_json_round_trips_without_backend_identity() {
    let overrides = request_override("archived", Some(true));
    let value = serde_json::to_value(&overrides).expect("serialize overrides");
    assert_eq!(
        value,
        serde_json::json!({
            "schema_version": 1,
            "excluded_operations": {},
            "operations": {
                "revise_job": {
                    "request_overrides": {
                        "archived": true
                    }
                }
            }
        })
    );
    let decoded: SdkOverrides = serde_json::from_value(value).expect("deserialize overrides");
    assert_eq!(decoded, overrides);
}
