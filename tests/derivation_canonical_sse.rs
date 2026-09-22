use std::collections::BTreeMap;

use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, OperationOverride,
    PublicSdkSurface, ResponseRepresentationDefinition, Runtime, SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-sse/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-sse/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-canonical-sse/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_canonical_sse_with_owned_public_wrappers_and_discriminator() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    for operation_id in ["watch_job", "subscribe_notifications"] {
        let outcome = &derivation.report.operations[operation_id];
        assert_eq!(outcome.status, DerivationStatus::Derived);
        assert_eq!(outcome.reason.code, "inference.structurally_proven");
    }

    let watch = &derivation.definition.resources["jobs"].operations["watch"];
    assert_eq!(watch.raw_method.as_deref(), Some("raw_watch_17"));
    assert_eq!(
        watch.response_representation,
        Some(ResponseRepresentationDefinition::EventStream)
    );
    let stream = watch.stream.as_ref().expect("canonical SSE stream");
    assert_eq!(stream.item, "OpaqueJobChunk4");
    assert_eq!(stream.wrapper.as_deref(), Some("WatchJobsStreamItem"));
    assert_eq!(stream.type_name, "WatchJobsStream");
    assert_eq!(
        watch
            .request_overrides
            .as_ref()
            .expect("canonical discriminator")["stream"],
        Some(true)
    );

    let request = &derivation.definition.models["WatchJobsRequest"];
    assert_eq!(request.raw.as_deref(), Some("OpaqueWatch8"));
    assert_eq!(request.exclude.as_deref(), Some(&["stream".to_owned()][..]));

    let watch_item = &derivation.definition.models["WatchJobsStreamItem"];
    assert_eq!(watch_item.schema.as_deref(), Some("JobChunk"));
    assert_eq!(watch_item.raw.as_deref(), Some("OpaqueJobChunk4"));
    assert_eq!(watch_item.borrowed, Some(false));

    let subscribe = &derivation.definition.resources["notifications"].operations["subscribe"];
    let subscribe_stream = subscribe.stream.as_ref().expect("envelope SSE stream");
    assert_eq!(subscribe_stream.item, "OpaqueNotification6");
    assert_eq!(
        subscribe_stream.wrapper.as_deref(),
        Some("SubscribeNotificationsStreamItem")
    );
    assert_eq!(subscribe_stream.type_name, "SubscribeNotificationsStream");

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");

    let types = &generated.files["facade_types.rs"];
    assert!(types.contains(
        "pub type WatchJobsStream = Pin<Box<dyn Stream<Item = Result<WatchJobsStreamItem, SdkError>> + Send + 'static>>;"
    ));
    assert!(types.contains(
        "pub type SubscribeNotificationsStream = Pin<Box<dyn Stream<Item = Result<SubscribeNotificationsStreamItem, SdkError>> + Send + 'static>>;"
    ));

    assert!(generated.files.values().any(|source| {
        source.contains(
            "pub async fn watch(&self, request: WatchJobsRequest, job_id: impl AsRef<str>) -> Result<WatchJobsStream, SdkError>"
        ) && source.contains("raw.stream = Some(true);")
            && source.contains("json_events::<_, _, OpaqueJobChunk4>(bytes)")
            && source.contains("WatchJobsStreamItem::from(event.data)")
    }));
    assert!(generated.files.values().any(|source| {
        source.contains("pub async fn subscribe(&self")
            && source.contains("last_event_id: Option<")
            && source.contains("SubscribeNotificationsStream")
    }));
    assert!(generated.files.values().any(|source| {
        source.contains("raw_notifications_31(")
            && source.contains("last_event_id")
            && source.contains("json_events::<_, _, OpaqueNotification6>(bytes)")
            && source.contains("SubscribeNotificationsStreamItem::from(event.data)")
    }));
}

#[test]
fn derives_exact_named_non_scalar_sse_payload() {
    let (mut openapi, mut bindings, surface) = fixture();

    *openapi
        .0
        .pointer_mut("/components/schemas/JobChunk/properties/message")
        .expect("job chunk message schema") =
        serde_json::json!({"type": "array", "items": {"type": "string"}});

    let mut fields = bindings
        .structs
        .remove("OpaqueJobChunk4")
        .expect("opaque job chunk binding");
    fields
        .iter_mut()
        .find(|field| field.name == "message")
        .expect("message binding")
        .type_name = "Vec<String>".into();
    bindings.structs.insert("JobChunk".into(), fields);
    if let Some(path) = bindings.symbol_paths.remove("OpaqueJobChunk4") {
        bindings.symbol_paths.insert(
            "JobChunk".into(),
            path.replace("OpaqueJobChunk4", "JobChunk"),
        );
    }

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive named complex SSE payload");
    assert_eq!(
        derivation.report.operations["watch_job"].status,
        DerivationStatus::Derived
    );
    assert_eq!(
        derivation.definition.models["WatchJobsStreamItem"]
            .raw
            .as_deref(),
        Some("JobChunk")
    );
    assert_eq!(
        derivation.definition.resources["jobs"].operations["watch"]
            .stream
            .as_ref()
            .expect("derived stream")
            .item,
        "JobChunk"
    );

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("named complex SSE payload should generate");

    assert!(generated.files.values().any(|source| {
        source.contains("json_events::<_, _, JobChunk>(bytes)")
            && source.contains("WatchJobsStreamItem::from(event.data)")
    }));
}

#[test]
fn derives_inline_standard_sse_envelope_with_optional_data() {
    let (mut openapi, bindings, surface) = fixture();
    let envelope = serde_json::json!({
        "type": "object",
        "properties": {
            "event": {"type": "string"},
            "data": {"$ref": "#/components/schemas/NotificationChunk"},
            "id": {"type": "string"},
            "retry": {"type": "integer"}
        }
    });
    for status in ["200", "206"] {
        *openapi
            .0
            .pointer_mut(&format!(
                "/paths/~1notifications/get/responses/{status}/content/text~1event-stream/schema"
            ))
            .expect("notification SSE schema") = envelope.clone();
    }

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("inline standard SSE envelope should derive");

    let outcome = &derivation.report.operations["subscribe_notifications"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");

    generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("inline standard SSE envelope should lower");
}

#[test]
fn lowering_revalidates_sse_envelope_payload() {
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
        .pointer_mut("/components/schemas/NotificationEnvelope/required")
        .expect("envelope required") = serde_json::json!([]);

    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect_err("SSE envelope drift must fail lowering");

    assert_eq!(error.diagnostic.code, "lower.stream_drift");
}

#[test]
fn rejects_unproven_sse_discriminator_shape() {
    let (openapi, mut bindings, surface) = fixture();
    let discriminator = bindings
        .operations
        .get_mut("raw_watch_17")
        .and_then(|binding| binding.metadata.as_mut())
        .and_then(|metadata| metadata.request_discriminators.first_mut())
        .expect("stream discriminator");
    discriminator.field_tri_state = true;

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["watch_job"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "capability.request_discriminator_projection_required"
    );
}

#[test]
fn consumer_override_cannot_replace_canonical_discriminator() {
    let (openapi, bindings, surface) = fixture();
    let mut overrides = SdkOverrides::default();
    overrides.operations.insert(
        "watch_job".into(),
        rust_sdk_generator::OperationOverride {
            response_representations: Default::default(),
            request_overrides: std::collections::BTreeMap::from([("stream".into(), Some(false))]),
        },
    );

    let error = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides,
    })
    .expect_err("consumer must not replace canonical stream discriminator");

    assert_eq!(error.diagnostic.code, "overrides.conflict");
}

#[test]
fn canonical_boolean_leaf_discriminator_uses_proven_optional_raw_field() {
    let (openapi, mut bindings, surface) = fixture();
    let discriminator = bindings
        .operations
        .get_mut("raw_watch_17")
        .and_then(|binding| binding.metadata.as_mut())
        .and_then(|metadata| metadata.request_discriminators.first_mut())
        .expect("stream discriminator");
    discriminator.rust_value_type = "bool".into();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    assert_eq!(
        derivation.report.operations["watch_job"].status,
        DerivationStatus::Derived
    );

    generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("canonical leaf discriminator must lower");
}

#[test]
fn canonical_discriminator_must_not_hide_raw_field_type_drift() {
    let (openapi, mut bindings, surface) = fixture();
    let discriminator = bindings
        .operations
        .get_mut("raw_watch_17")
        .and_then(|binding| binding.metadata.as_mut())
        .and_then(|metadata| metadata.request_discriminators.first_mut())
        .expect("stream discriminator");
    discriminator.rust_value_type = "bool".into();
    bindings
        .structs
        .get_mut("OpaqueWatch8")
        .expect("request model")
        .iter_mut()
        .find(|field| field.name == "stream")
        .expect("stream field")
        .type_name = "bool".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");
    assert_eq!(
        derivation.report.operations["watch_job"].status,
        DerivationStatus::Rejected
    );
}

#[test]
fn explicit_json_and_sse_selection_preserves_canonical_discriminators() {
    let (mut openapi, mut bindings, mut surface) = fixture();
    openapi.0["paths"]["/jobs/{job_id}/watch"]["post"]["responses"]["201"] = serde_json::json!({
        "description": "buffered result",
        "content": {"application/json": {
            "schema": {"$ref": "#/components/schemas/BufferedJob"}
        }}
    });
    openapi.0["components"]["schemas"]["BufferedJob"] = serde_json::json!({
        "type": "object",
        "required": ["result"],
        "properties": {"result": {"type": "boolean"}}
    });
    bindings.structs.insert(
        "BufferedJob".into(),
        vec![rust_sdk_generator::FieldBinding {
            name: "result".into(),
            wire_name: Some("result".into()),
            type_name: "bool".into(),
        }],
    );
    bindings.symbol_paths.insert(
        "BufferedJob".into(),
        "crate::generated::types::BufferedJob".into(),
    );
    let mut json = bindings.operations["raw_watch_17"].clone();
    json.name = "raw_read_job_18".into();
    json.success_type = "BufferedJob".into();
    json.return_type = "Result<BufferedJob, Error>".into();
    json.stream = None;
    let metadata = json.metadata.as_mut().expect("canonical metadata");
    metadata.representation = rust_sdk_generator::ResponseRepresentationBinding::Json {
        schema_name: "BufferedJob".into(),
        media_type: "application/json".into(),
    };
    metadata.success_statuses = vec!["201".into()];
    metadata.stream_abi = None;
    metadata.request_discriminators[0].value =
        rust_sdk_generator::RequestDiscriminatorValue::Bool(false);
    bindings.operations.insert(json.name.clone(), json);
    surface.operations.insert(
        "watch_job".into(),
        vec!["jobs.read".into(), "jobs.watch".into()],
    );

    let mut overrides = SdkOverrides::default();
    overrides.operations.insert(
        "watch_job".into(),
        OperationOverride {
            request_overrides: BTreeMap::new(),
            response_representations: BTreeMap::from([
                ("jobs.read".into(), ResponseRepresentationDefinition::Json),
                (
                    "jobs.watch".into(),
                    ResponseRepresentationDefinition::EventStream,
                ),
            ]),
        },
    );
    let derived = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides,
    })
    .expect("canonical JSON and SSE projection");
    assert_eq!(
        derived.report.operations["watch_job"].status,
        DerivationStatus::Overridden
    );
    let ops = &derived.definition.resources["jobs"].operations;
    assert_eq!(ops["read"].raw_method.as_deref(), Some("raw_read_job_18"));
    assert_eq!(ops["watch"].raw_method.as_deref(), Some("raw_watch_17"));
    assert_eq!(
        ops["read"]
            .request_overrides
            .as_ref()
            .expect("JSON discriminator")["stream"],
        Some(false)
    );
    assert_eq!(
        ops["watch"]
            .request_overrides
            .as_ref()
            .expect("SSE discriminator")["stream"],
        Some(true)
    );

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derived.definition,
        runtime: Runtime::default(),
    })
    .expect("selected JSON and SSE lower");
    assert!(
        generated
            .files
            .values()
            .any(|source| source.contains("raw_read_job_18("))
    );
    assert!(
        generated
            .files
            .values()
            .any(|source| source.contains("raw_watch_17("))
    );
}

#[test]
fn canonical_discriminator_proves_matching_boolean_request_constant() {
    let (mut openapi, bindings, surface) = fixture();
    *openapi
        .0
        .pointer_mut("/components/schemas/WatchRequest/properties/stream")
        .expect("stream schema") = serde_json::json!({
        "type": "boolean",
        "const": true,
        "default": true
    });

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("matching canonical Boolean discriminator should prove request constant");

    assert_eq!(
        derivation.report.operations["watch_job"].status,
        DerivationStatus::Derived
    );
    let request = &derivation.definition.models["WatchJobsRequest"];
    assert_eq!(request.raw.as_deref(), Some("OpaqueWatch8"));
    assert!(request.constructor.is_none());
    assert_eq!(request.exclude.as_deref(), Some(&["stream".to_owned()][..]));

    generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("discriminator-proven request constant must lower");
}

#[test]
fn canonical_discriminator_rejects_conflicting_boolean_request_constant() {
    let (mut openapi, mut bindings, surface) = fixture();
    *openapi
        .0
        .pointer_mut("/components/schemas/WatchRequest/properties/stream")
        .expect("stream schema") = serde_json::json!({
        "type": "boolean",
        "const": true,
        "default": true
    });
    bindings
        .operations
        .get_mut("raw_watch_17")
        .and_then(|binding| binding.metadata.as_mut())
        .and_then(|metadata| metadata.request_discriminators.first_mut())
        .expect("stream discriminator")
        .value = rust_sdk_generator::RequestDiscriminatorValue::Bool(false);

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    assert_eq!(
        derivation.report.operations["watch_job"].status,
        DerivationStatus::Rejected
    );
    assert_eq!(
        derivation.report.operations["watch_job"].reason.code,
        "bindings.no_structural_match"
    );
}
