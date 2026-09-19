use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, SimpleUnionVariant, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-union/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-union/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-union/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_inline_object_union_by_shape_not_raw_order_or_names() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["read_latest_event"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");
    assert_eq!(outcome.binding.as_deref(), Some("call_union_9"));

    let union = &derivation.definition.models["LatestEventsResponse"];
    assert_eq!(union.raw.as_deref(), Some("OpaqueUnion3"));
    let simple = union.simple_union.as_ref().expect("inline simple union");
    assert!(simple.bidirectional);
    assert_eq!(
        simple.variants["RawA"],
        SimpleUnionVariant::Adapted {
            name: "Variant1".into(),
            adapter: "LatestEventsResponseVariant1".into(),
        }
    );
    assert_eq!(
        simple.variants["RawB"],
        SimpleUnionVariant::Adapted {
            name: "Variant2".into(),
            adapter: "LatestEventsResponseVariant2".into(),
        }
    );

    let first = &derivation.definition.models["LatestEventsResponseVariant1"];
    assert_eq!(first.raw.as_deref(), Some("OpaquePayload17"));
    assert_eq!(first.borrowed, Some(false));
    assert_eq!(
        first
            .accessors
            .as_ref()
            .expect("first branch accessors")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["message"]
    );

    let second = &derivation.definition.models["LatestEventsResponseVariant2"];
    assert_eq!(second.raw.as_deref(), Some("OpaquePayload42"));
    assert_eq!(second.borrowed, Some(false));
    assert_eq!(
        second
            .accessors
            .as_ref()
            .expect("second branch accessors")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["code", "retryable"]
    );

    let operation = &derivation.definition.resources["events"].operations["latest"];
    assert_eq!(operation.raw_method.as_deref(), Some("call_union_9"));
    assert_eq!(operation.response.as_deref(), Some("LatestEventsResponse"));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    assert_eq!(generated.inventory.client, "EventsClient");
    assert!(
        generated
            .inventory
            .models
            .contains(&"LatestEventsResponse".to_owned())
    );
    assert!(
        generated
            .inventory
            .models
            .contains(&"LatestEventsResponseVariant1".to_owned())
    );
    assert!(
        generated
            .inventory
            .models
            .contains(&"LatestEventsResponseVariant2".to_owned())
    );

    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("impl From<LatestEventsResponse> for OpaqueUnion3"));
    assert!(types.contains("impl From<OpaqueUnion3> for LatestEventsResponse"));
    assert!(types.contains("OpaqueUnion3::RawA(value) => Self::Variant1(value.into())"));
    assert!(types.contains("OpaqueUnion3::RawB(value) => Self::Variant2(value.into())"));
}

#[test]
fn rejects_inline_union_payload_shape_drift_during_reconciliation() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .structs
        .get_mut("OpaquePayload17")
        .expect("first payload")[0]
        .type_name = "i64".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["read_latest_event"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn rejects_ambiguous_inline_union_branch_shapes() {
    let (mut openapi, bindings, surface) = fixture();
    let branches = openapi
        .0
        .pointer_mut(
            "/paths/~1events~1latest/get/responses/200/content/application~1json/schema/oneOf",
        )
        .and_then(serde_json::Value::as_array_mut)
        .expect("inline union branches");
    branches[1] = branches[0].clone();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["read_latest_event"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn rejects_inline_union_public_model_name_collision() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .structs
        .insert("LatestEventsResponse".into(), Vec::new());
    bindings.symbol_paths.insert(
        "LatestEventsResponse".into(),
        "crate::generated::types::LatestEventsResponse".into(),
    );

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["read_latest_event"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "capability.public_model_name_collision"
    );
}

#[test]
fn preserves_recursive_inline_union_as_owned_raw_view() {
    let (mut openapi, mut bindings, surface) = fixture();
    openapi.0["paths"]["/events/latest"]["get"]["responses"]["200"]["content"]["application/json"]
        ["schema"] = serde_json::json!({
        "anyOf": [
            {"type": "array", "items": {"type": "string"}},
            {"type": "array", "items": {"type": "integer"}}
        ],
        "title": "Recursive inline union"
    });
    bindings.structs.remove("OpaquePayload17");
    bindings.structs.remove("OpaquePayload42");
    bindings
        .aliases
        .insert("OpaqueList17".into(), "Vec<String>".into());
    bindings
        .aliases
        .insert("OpaqueList42".into(), "Vec<i64>".into());
    let variants = bindings.enums.get_mut("OpaqueUnion3").expect("raw union");
    for variant in variants {
        variant.payload = Some(
            match variant.name.as_str() {
                "RawA" => "OpaqueList17",
                "RawB" => "OpaqueList42",
                other => panic!("unexpected raw variant {other}"),
            }
            .into(),
        );
    }
    bindings.symbol_paths.remove("OpaquePayload17");
    bindings.symbol_paths.remove("OpaquePayload42");
    bindings.symbol_paths.insert(
        "OpaqueList17".into(),
        "crate::generated::types::OpaqueList17".into(),
    );
    bindings.symbol_paths.insert(
        "OpaqueList42".into(),
        "crate::generated::types::OpaqueList42".into(),
    );

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive recursive inline union");

    assert_eq!(
        derivation.report.operations["read_latest_event"].status,
        DerivationStatus::Derived
    );
    let response = &derivation.definition.models["LatestEventsResponse"];
    assert_eq!(response.raw.as_deref(), Some("OpaqueUnion3"));
    assert!(response.simple_union.is_none());
    assert_eq!(response.borrowed, Some(false));
    assert!(
        response
            .accessors
            .as_ref()
            .is_some_and(indexmap::IndexMap::is_empty)
    );

    let definition = derivation.definition.clone();
    let generated = generate(GenerateInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("recursive inline union raw view generates");
    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("pub struct LatestEventsResponse { raw: OpaqueUnion3 }"));
    assert!(!types.contains("pub enum LatestEventsResponse"));

    openapi.0["paths"]["/events/latest"]["get"]["responses"]["200"]["content"]["application/json"]
        ["schema"]["anyOf"][1]["items"] = serde_json::json!({"type": "boolean"});
    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition,
        runtime: Runtime::default(),
    })
    .expect_err("recursive inline union drift must fail lowering");
    assert_eq!(error.diagnostic.code, "lower.response_drift");
}
