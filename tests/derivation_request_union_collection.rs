use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, SimpleUnionVariant, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-request-union-collection/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-request-union-collection/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-request-union-collection/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_request_union_collection_by_structure_not_raw_names_or_order() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["send_batch"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");
    assert_eq!(outcome.binding.as_deref(), Some("opaque_send_31"));

    let root = &derivation.definition.models["SendDeliveryCommandsRequest"];
    assert_eq!(root.schema.as_deref(), Some("SubmitBatchRequest"));
    assert_eq!(root.raw.as_deref(), Some("OpaqueBatch9"));
    assert_eq!(
        root.constructor.as_deref(),
        Some(&["label".to_owned(), "commands".to_owned()][..])
    );
    assert_eq!(
        root.adapters.as_ref().expect("root adapters")["commands"],
        "SendDeliveryCommandsRequestCommands"
    );

    let union = &derivation.definition.models["SendDeliveryCommandsRequestCommands"];
    assert_eq!(union.raw.as_deref(), Some("OpaqueCommand7"));
    assert_eq!(union.schema.as_deref(), Some("SubmitBatchRequest"));
    assert_eq!(
        union.schema_path.as_deref(),
        Some(&["commands".to_owned(), "items".to_owned()][..])
    );
    let simple = union.simple_union.as_ref().expect("request simple union");
    assert!(!simple.bidirectional);
    assert_eq!(
        simple.variants["VariantA"],
        SimpleUnionVariant::Adapted {
            name: "EmailCommand".into(),
            adapter: "SendDeliveryCommandsRequestCommandsEmailCommand".into(),
        }
    );
    assert_eq!(
        simple.variants["VariantB"],
        SimpleUnionVariant::Adapted {
            name: "SmsCommand".into(),
            adapter: "SendDeliveryCommandsRequestCommandsSmsCommand".into(),
        }
    );

    let email = &derivation.definition.models["SendDeliveryCommandsRequestCommandsEmailCommand"];
    assert_eq!(email.schema.as_deref(), Some("EmailCommand"));
    assert_eq!(email.raw.as_deref(), Some("OpaqueEmail3"));
    assert_eq!(
        email.constructor.as_deref(),
        Some(&["address".to_owned(), "subject".to_owned()][..])
    );

    let sms = &derivation.definition.models["SendDeliveryCommandsRequestCommandsSmsCommand"];
    assert_eq!(sms.schema.as_deref(), Some("SmsCommand"));
    assert_eq!(sms.raw.as_deref(), Some("OpaqueSms4"));

    let operation = &derivation.definition.resources["delivery_commands"].operations["send"];
    assert_eq!(operation.raw_method.as_deref(), Some("opaque_send_31"));
    assert_eq!(
        operation.request.as_deref(),
        Some("SendDeliveryCommandsRequest")
    );
    assert_eq!(operation.empty_response, Some(true));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    let types = &generated.files["facade_types.rs"];
    assert!(
        types.contains("commands: impl IntoIterator<Item = SendDeliveryCommandsRequestCommands>")
    );
    assert!(types.contains("commands.into_iter().map(Into::into).collect()"));
    assert!(types.contains("pub enum SendDeliveryCommandsRequestCommands"));
    assert!(types.contains("EmailCommand(SendDeliveryCommandsRequestCommandsEmailCommand)"));
    assert!(types.contains("SmsCommand(SendDeliveryCommandsRequestCommandsSmsCommand)"));
    assert!(types.contains(
        "impl From<SendDeliveryCommandsRequestCommandsEmailCommand> for SendDeliveryCommandsRequestCommands"
    ));
    assert!(types.contains(
        "impl From<SendDeliveryCommandsRequestCommandsSmsCommand> for SendDeliveryCommandsRequestCommands"
    ));
    assert!(types.contains("impl From<SendDeliveryCommandsRequestCommands> for OpaqueCommand7"));
    assert!(types.contains(
        "SendDeliveryCommandsRequestCommands::EmailCommand(value) => Self::VariantA(value.into())"
    ));
    assert!(types.contains(
        "SendDeliveryCommandsRequestCommands::SmsCommand(value) => Self::VariantB(value.into())"
    ));
}

#[test]
fn rejects_request_union_payload_drift_during_reconciliation() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .structs
        .get_mut("OpaqueEmail3")
        .expect("email binding")[2]
        .type_name = "bool".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["send_batch"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn rejects_ambiguous_request_union_branch_shapes() {
    let (mut openapi, mut bindings, surface) = fixture();
    let email = openapi
        .0
        .pointer("/components/schemas/EmailCommand")
        .expect("email schema")
        .clone();
    *openapi
        .0
        .pointer_mut("/components/schemas/SmsCommand")
        .expect("sms schema") = email;
    let email_fields = bindings.structs["OpaqueEmail3"].clone();
    bindings.structs.insert("OpaqueSms4".into(), email_fields);

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["send_batch"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn preserves_optional_request_union_branch_evolution() {
    let (mut openapi, mut bindings, surface) = fixture();
    openapi
        .0
        .pointer_mut("/components/schemas/EmailCommand/properties")
        .and_then(serde_json::Value::as_object_mut)
        .expect("email properties")
        .insert("tracking".into(), serde_json::json!({"type": "string"}));
    bindings
        .structs
        .get_mut("OpaqueEmail3")
        .expect("email binding")
        .push(rust_sdk_generator::FieldBinding {
            name: "tracking".into(),
            type_name: "Option<String>".into(),
        });

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    assert_eq!(
        derivation.report.operations["send_batch"].status,
        DerivationStatus::Derived
    );
}

#[test]
fn lowering_revalidates_request_union_collection_drift() {
    let (mut openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    *openapi
        .0
        .pointer_mut("/components/schemas/EmailCommand/properties/address")
        .expect("email address schema") = serde_json::json!({"type": "boolean"});

    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect_err("request union collection drift must fail lowering");
    assert_eq!(error.diagnostic.code, "lower.request_union_drift");
}
