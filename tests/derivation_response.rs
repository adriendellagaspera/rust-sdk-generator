use rust_sdk_generator::{
    AccessorKindDefinition, Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi,
    PublicSdkSurface, Runtime, SdkOverrides, derive, generate,
};

#[test]
fn derives_owned_scalar_response_view_and_generates_it() {
    let openapi: OpenApi =
        serde_json::from_str(include_str!("fixtures/derivation-response/openapi.json"))
            .expect("fixture OpenAPI");
    let bindings: Bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-response/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface: PublicSdkSurface =
        serde_json::from_str(include_str!("fixtures/derivation-response/surface.json"))
            .expect("fixture surface");

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["read_sensor"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");
    assert_eq!(outcome.binding.as_deref(), Some("opaque_read"));

    let response = &derivation.definition.models["GetFleetSensorsResponse"];
    assert_eq!(response.raw.as_deref(), Some("SensorResponse"));
    assert_eq!(response.borrowed, Some(false));
    let accessors = response
        .accessors
        .as_ref()
        .expect("response view accessors");
    assert_eq!(
        accessors.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["active", "count", "name", "note", "temperature"]
    );
    assert_eq!(accessors["active"].kind, AccessorKindDefinition::Copy);
    assert_eq!(accessors["count"].kind, AccessorKindDefinition::Copy);
    assert_eq!(accessors["name"].kind, AccessorKindDefinition::Ref);
    assert_eq!(accessors["note"].kind, AccessorKindDefinition::OptionalRef);
    assert_eq!(accessors["temperature"].kind, AccessorKindDefinition::Copy);

    let operation = &derivation.definition.resources["fleet_sensors"].operations["get"];
    assert_eq!(operation.raw_method.as_deref(), Some("opaque_read"));
    assert_eq!(
        operation.response.as_deref(),
        Some("GetFleetSensorsResponse")
    );
    assert_eq!(operation.empty_response, None);

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");
    assert_eq!(generated.inventory.client, "FleetClient");
    assert_eq!(generated.inventory.models, vec!["GetFleetSensorsResponse"]);
    assert_eq!(generated.inventory.resources.len(), 2);
    assert_eq!(
        generated.inventory.resources[1].path,
        vec!["fleet", "sensors"]
    );
    assert_eq!(generated.inventory.resources[1].operations, vec!["get"]);
}

#[test]
fn derives_closed_empty_response_view_but_not_an_open_map() {
    let mut openapi: OpenApi =
        serde_json::from_str(include_str!("fixtures/derivation-response/openapi.json"))
            .expect("fixture OpenAPI");
    let mut bindings: Bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-response/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface: PublicSdkSurface =
        serde_json::from_str(include_str!("fixtures/derivation-response/surface.json"))
            .expect("fixture surface");

    openapi.0["components"]["schemas"]["SensorResponse"] = serde_json::json!({
        "type": "object",
        "additionalProperties": false
    });
    bindings.structs.insert("SensorResponse".into(), vec![]);

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface: surface.clone(),
        overrides: SdkOverrides::default(),
    })
    .expect("derive closed empty response");
    assert_eq!(
        derivation.report.operations["read_sensor"].status,
        DerivationStatus::Derived
    );
    let response = &derivation.definition.models["GetFleetSensorsResponse"];
    assert_eq!(response.raw.as_deref(), Some("SensorResponse"));
    assert_eq!(response.borrowed, Some(false));
    assert!(
        response
            .accessors
            .as_ref()
            .is_some_and(|items| items.is_empty())
    );
    generate(GenerateInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("closed empty response view generates");

    openapi.0["components"]["schemas"]["SensorResponse"]["additionalProperties"] =
        serde_json::json!(true);
    let open_map = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("open map is classified without guessing a closed view");
    assert_eq!(
        open_map.report.operations["read_sensor"].status,
        DerivationStatus::Rejected
    );
}
