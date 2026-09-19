use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface,
    ResponseRepresentationDefinition, Runtime, SdkOverrides, derive, generate,
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
        source.contains(
            "pub async fn subscribe(&self, last_event_id: Option<impl AsRef<str>>) -> Result<SubscribeNotificationsStream, SdkError>",
        ) && source.contains("raw_notifications_31(")
            && source.contains("last_event_id")
            && source.contains("json_events::<_, _, OpaqueNotification6>(bytes)")
            && source.contains("SubscribeNotificationsStreamItem::from(event.data)")
    }));
}

#[test]
fn explicit_sse_accepts_exact_named_non_scalar_payload() {
    let (mut openapi, mut bindings, surface) = fixture();
    let mut definition = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive")
    .definition;

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

    definition.models["WatchJobsStreamItem"].raw = Some("JobChunk".into());
    definition.resources["jobs"].operations["watch"]
        .stream
        .as_mut()
        .expect("explicit stream definition")
        .item = "JobChunk".into();

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition,
        runtime: Runtime::default(),
    })
    .expect("exact named SSE payload should reconcile");

    assert!(generated.files.values().any(|source| {
        source.contains("json_events::<_, _, JobChunk>(bytes)")
            && source.contains("WatchJobsStreamItem::from(event.data)")
    }));
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
