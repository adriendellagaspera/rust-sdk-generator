use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-nested-request/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-nested-request/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-nested-request/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_nested_named_request_models_without_raw_name_identity() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["create_widget"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");
    assert_eq!(outcome.binding.as_deref(), Some("opaque_create_call"));

    let root = &derivation.definition.models["CreatePlatformWidgetsRequest"];
    assert_eq!(root.schema.as_deref(), Some("CreateWidgetRequest"));
    assert_eq!(root.raw.as_deref(), Some("OpaqueRequest9"));
    assert_eq!(
        root.constructor.as_deref(),
        Some(&["name".to_owned(), "config".to_owned()][..])
    );
    let adapters = root.adapters.as_ref().expect("nested adapters");
    assert_eq!(adapters["config"], "CreatePlatformWidgetsRequestConfig");
    assert_eq!(adapters["metadata"], "CreatePlatformWidgetsRequestMetadata");

    let config = &derivation.definition.models["CreatePlatformWidgetsRequestConfig"];
    assert_eq!(config.schema.as_deref(), Some("WidgetConfig"));
    assert_eq!(config.raw.as_deref(), Some("OpaqueConfig4"));
    assert_eq!(
        config.constructor.as_deref(),
        Some(&["mode".to_owned()][..])
    );
    assert!(
        config
            .adapters
            .as_ref()
            .is_none_or(indexmap::IndexMap::is_empty)
    );
    assert!(
        !derivation
            .definition
            .models
            .contains_key("CreatePlatformWidgetsRequestConfigLabels")
    );
    assert!(
        !derivation
            .definition
            .models
            .contains_key("CreatePlatformWidgetsRequestSelector")
    );

    let metadata = &derivation.definition.models["CreatePlatformWidgetsRequestMetadata"];
    assert_eq!(metadata.schema.as_deref(), Some("WidgetMetadata"));
    assert_eq!(metadata.raw.as_deref(), Some("OpaqueMeta7"));
    assert_eq!(metadata.constructor.as_deref(), Some(&[][..]));

    let operation = &derivation.definition.resources["platform_widgets"].operations["create"];
    assert_eq!(operation.raw_method.as_deref(), Some("opaque_create_call"));
    assert_eq!(
        operation.request.as_deref(),
        Some("CreatePlatformWidgetsRequest")
    );
    assert_eq!(operation.empty_response, Some(true));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    assert_eq!(generated.inventory.client, "PlatformClient");
    for expected in [
        "CreatePlatformWidgetsRequest",
        "CreatePlatformWidgetsRequestConfig",
        "CreatePlatformWidgetsRequestMetadata",
    ] {
        assert!(generated.inventory.models.contains(&expected.to_owned()));
    }

    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("pub struct CreatePlatformWidgetsRequest { raw: OpaqueRequest9 }"));
    assert!(types.contains("config: impl Into<CreatePlatformWidgetsRequestConfig>"));
    assert!(types.contains(
        "pub fn metadata(mut self, metadata: impl Into<CreatePlatformWidgetsRequestMetadata>)"
    ));
    assert!(types.contains("pub struct CreatePlatformWidgetsRequestConfig { raw: OpaqueConfig4 }"));
    assert!(types.contains("pub struct CreatePlatformWidgetsRequestMetadata { raw: OpaqueMeta7 }"));
}

#[test]
fn rejects_nested_request_shape_drift_during_reconciliation() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .structs
        .get_mut("OpaqueConfig4")
        .expect("config binding")[1]
        .type_name = "bool".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["create_widget"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn rejects_recursive_named_request_shapes_deterministically() {
    let (mut openapi, mut bindings, surface) = fixture();
    let metadata = openapi
        .0
        .pointer_mut("/components/schemas/WidgetMetadata")
        .and_then(serde_json::Value::as_object_mut)
        .expect("metadata schema");
    metadata.insert("required".into(), serde_json::json!(["parent"]));
    metadata.insert(
        "properties".into(),
        serde_json::json!({
            "parent": {"$ref": "#/components/schemas/CreateWidgetRequest"}
        }),
    );
    bindings.structs.insert(
        "OpaqueMeta7".into(),
        vec![rust_sdk_generator::FieldBinding {
            name: "parent".into(),
            wire_name: None,
            type_name: "OpaqueRequest9".into(),
        }],
    );

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["create_widget"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn derives_annotation_only_json_request_field_without_guessing_a_scalar_type() {
    let (mut openapi, mut bindings, surface) = fixture();
    openapi.0["components"]["schemas"]["CreateWidgetRequest"]["properties"]["payload"] =
        serde_json::json!({"title": "Payload", "description": "Arbitrary JSON"});
    openapi.0["components"]["schemas"]["CreateWidgetRequest"]["required"]
        .as_array_mut()
        .expect("required fields")
        .push(serde_json::json!("payload"));
    bindings
        .structs
        .get_mut("OpaqueRequest9")
        .expect("request fields")
        .push(rust_sdk_generator::FieldBinding {
            name: "payload".into(),
            wire_name: Some("payload".into()),
            type_name: "serde_json::Value".into(),
        });

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    assert_eq!(
        derivation.report.operations["create_widget"].status,
        DerivationStatus::Derived
    );
    assert!(
        generate(GenerateInput {
            openapi,
            bindings,
            definition: derivation.definition,
            runtime: Runtime::default(),
        })
        .is_ok()
    );
}

#[test]
fn rejects_typed_request_field_against_unconstrained_raw_json_value() {
    let (mut openapi, mut bindings, surface) = fixture();
    openapi.0["components"]["schemas"]["CreateWidgetRequest"]["properties"]["payload"] =
        serde_json::json!({"type": "string"});
    openapi.0["components"]["schemas"]["CreateWidgetRequest"]["required"]
        .as_array_mut()
        .expect("required fields")
        .push(serde_json::json!("payload"));
    bindings
        .structs
        .get_mut("OpaqueRequest9")
        .expect("request fields")
        .push(rust_sdk_generator::FieldBinding {
            name: "payload".into(),
            wire_name: Some("payload".into()),
            type_name: "serde_json::Value".into(),
        });

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("closed-world derivation");
    assert_eq!(
        derivation.report.operations["create_widget"].status,
        DerivationStatus::Rejected
    );
}

#[test]
fn derives_one_variant_string_const_request_fields_without_name_inference() {
    let (mut openapi, mut bindings, surface) = fixture();
    openapi.0["components"]["schemas"]["CreateWidgetRequest"]["properties"]["kind"] =
        serde_json::json!({"type": "string", "const": "widget", "title": "Kind"});
    bindings
        .structs
        .get_mut("OpaqueRequest9")
        .expect("request")
        .push(rust_sdk_generator::FieldBinding {
            name: "kind".into(),
            wire_name: Some("kind".into()),
            type_name: "Option<OpaqueKind>".into(),
        });
    bindings.enums.insert(
        "OpaqueKind".into(),
        serde_json::from_value(serde_json::json!([{
            "name": "Widget",
            "wire_name": "widget",
            "payload": null
        }]))
        .expect("single-variant enum"),
    );
    bindings.symbol_paths.insert(
        "OpaqueKind".into(),
        "crate::generated::types::OpaqueKind".into(),
    );

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface: surface.clone(),
        overrides: SdkOverrides::default(),
    })
    .expect("derive const enum");
    assert_eq!(
        derivation.report.operations["create_widget"].status,
        DerivationStatus::Derived
    );
    generate(GenerateInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("generate const enum");

    let mut bad_value = openapi.clone();
    bad_value.0["components"]["schemas"]["CreateWidgetRequest"]["properties"]["kind"]["const"] =
        serde_json::json!("other");
    let outcome = derive(DeriveInput {
        openapi: bad_value,
        bindings: bindings.clone(),
        surface: surface.clone(),
        overrides: SdkOverrides::default(),
    })
    .expect("closed-world derivation");
    assert_eq!(
        outcome.report.operations["create_widget"].status,
        DerivationStatus::Rejected
    );

    let mut unconstrained = bindings.clone();
    unconstrained
        .structs
        .get_mut("OpaqueRequest9")
        .expect("request")
        .last_mut()
        .expect("kind")
        .type_name = "Option<String>".into();
    let outcome = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: unconstrained,
        surface: surface.clone(),
        overrides: SdkOverrides::default(),
    })
    .expect("closed-world derivation");
    assert_eq!(
        outcome.report.operations["create_widget"].status,
        DerivationStatus::Rejected
    );

    let mut bad_format = openapi;
    bad_format.0["components"]["schemas"]["CreateWidgetRequest"]["properties"]["kind"]["format"] =
        serde_json::json!("binary");
    let outcome = derive(DeriveInput {
        openapi: bad_format,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("closed-world derivation");
    assert_eq!(
        outcome.report.operations["create_widget"].status,
        DerivationStatus::Rejected
    );
}

#[test]
fn derives_nested_flattened_json_as_a_proven_raw_request_field() {
    let (mut openapi, mut bindings, surface) = fixture();
    openapi.0["components"]["schemas"]["FlexiblePayload"] = serde_json::json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {"type": "object", "additionalProperties": true}
            }
        },
        "required": ["items"],
        "additionalProperties": true
    });
    openapi.0["components"]["schemas"]["CreateWidgetRequest"]["properties"]["payload"] =
        serde_json::json!({"$ref": "#/components/schemas/FlexiblePayload"});
    openapi.0["components"]["schemas"]["CreateWidgetRequest"]["required"]
        .as_array_mut()
        .expect("required fields")
        .push(serde_json::json!("payload"));
    bindings
        .structs
        .get_mut("OpaqueRequest9")
        .expect("request fields")
        .push(rust_sdk_generator::FieldBinding {
            name: "payload".into(),
            wire_name: Some("payload".into()),
            type_name: "FlexiblePayload".into(),
        });
    bindings.structs.insert(
        "FlexiblePayload".into(),
        serde_json::from_value(serde_json::json!([
            {"name": "items", "wire_name": "items", "type": "Vec<OpaqueItem>"},
            {"name": "additional_properties", "wire_name": null,
             "type": "std::collections::BTreeMap<String, serde_json::Value>"}
        ]))
        .expect("payload"),
    );
    bindings.structs.insert(
        "OpaqueItem".into(),
        serde_json::from_value(serde_json::json!([
            {"name": "additional_properties", "wire_name": null,
             "type": "std::collections::BTreeMap<String, serde_json::Value>"}
        ]))
        .expect("nested item"),
    );
    for raw in ["FlexiblePayload", "OpaqueItem"] {
        bindings
            .symbol_paths
            .insert(raw.into(), format!("crate::generated::types::{raw}"));
    }

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface: surface.clone(),
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    assert_eq!(
        derivation.report.operations["create_widget"].status,
        DerivationStatus::Derived
    );
    let root = &derivation.definition.models["CreatePlatformWidgetsRequest"];
    assert!(
        root.adapters
            .as_ref()
            .is_none_or(|adapters| !adapters.contains_key("payload"))
    );
    let emitted = generate(GenerateInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("nested canonical JSON map request generation");
    assert!(emitted.files["facade_types.rs"].contains("payload: FlexiblePayload"));

    for extra in [
        serde_json::json!(false),
        serde_json::json!({"type": "string"}),
    ] {
        let mut drifted = openapi.clone();
        drifted.0["components"]["schemas"]["FlexiblePayload"]["additionalProperties"] = extra;
        let result = derive(DeriveInput {
            openapi: drifted,
            bindings: bindings.clone(),
            surface: surface.clone(),
            overrides: SdkOverrides::default(),
        })
        .expect("closed-world derivation");
        assert_eq!(
            result.report.operations["create_widget"].status,
            DerivationStatus::Rejected
        );
    }
    let mut drifted_bindings = bindings;
    drifted_bindings
        .structs
        .get_mut("FlexiblePayload")
        .expect("payload")[1]
        .type_name = "std::collections::BTreeMap<String, String>".into();
    let result = derive(DeriveInput {
        openapi,
        bindings: drifted_bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("closed-world derivation");
    assert_eq!(
        result.report.operations["create_widget"].status,
        DerivationStatus::Rejected
    );
}

#[test]
fn derives_nested_all_of_request_objects_by_composed_wire_shape() {
    let (mut openapi, bindings, surface) = fixture();
    let config = openapi.0["components"]["schemas"]["WidgetConfig"].clone();
    openapi.0["components"]["schemas"]["WidgetConfigBase"] = serde_json::json!({
        "type": "object",
        "required": ["mode"],
        "properties": {"mode": config["properties"]["mode"].clone()}
    });
    openapi.0["components"]["schemas"]["WidgetConfig"] = serde_json::json!({
        "allOf": [
            {"$ref": "#/components/schemas/WidgetConfigBase"},
            {"type": "object", "properties": {
                "retries": config["properties"]["retries"].clone(),
                "labels": config["properties"]["labels"].clone()
            }}
        ]
    });

    let result = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface: surface.clone(),
        overrides: SdkOverrides::default(),
    })
    .expect("composed named request derivation");
    assert_eq!(
        result.report.operations["create_widget"].status,
        DerivationStatus::Derived
    );
    let root = &result.definition.models["CreatePlatformWidgetsRequest"];
    assert_eq!(
        root.adapters.as_ref().expect("adapters")["config"],
        "CreatePlatformWidgetsRequestConfig"
    );
    let nested = &result.definition.models["CreatePlatformWidgetsRequestConfig"];
    assert_eq!(nested.schema.as_deref(), Some("WidgetConfig"));
    assert_eq!(
        nested.constructor.as_deref(),
        Some(&["mode".to_owned()][..])
    );
    assert!(
        generate(GenerateInput {
            openapi: openapi.clone(),
            bindings: bindings.clone(),
            definition: result.definition,
            runtime: Runtime::default(),
        })
        .is_ok()
    );

    let mut type_drift = openapi.clone();
    type_drift.0["components"]["schemas"]["WidgetConfig"]["allOf"][1]["properties"]["retries"] =
        serde_json::json!({"type": "string"});
    let drifted = derive(DeriveInput {
        openapi: type_drift,
        bindings: bindings.clone(),
        surface: surface.clone(),
        overrides: SdkOverrides::default(),
    })
    .expect("closed-world request derivation");
    assert_eq!(
        drifted.report.operations["create_widget"].status,
        DerivationStatus::Rejected
    );

    let mut recursion = openapi;
    recursion.0["components"]["schemas"]["WidgetConfigBase"] = serde_json::json!({
        "allOf": [{"$ref": "#/components/schemas/WidgetConfig"}]
    });
    let rejected = derive(DeriveInput {
        openapi: recursion,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("closed-world request derivation");
    assert_eq!(
        rejected.report.operations["create_widget"].status,
        DerivationStatus::Rejected
    );
}
