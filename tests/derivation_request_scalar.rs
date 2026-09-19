use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!("fixtures/derivation-update/openapi.json"))
        .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-update/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!("fixtures/derivation-update/surface.json"))
        .expect("fixture surface");
    (openapi, bindings, surface)
}

fn request_field_mut<'a>(bindings: &'a mut Bindings, name: &str) -> &'a mut String {
    &mut bindings
        .structs
        .get_mut("UpdateJobRequest")
        .expect("request binding")
        .iter_mut()
        .find(|field| field.name == name)
        .expect("request field")
        .type_name
}

#[test]
fn rejects_required_request_scalar_type_drift() {
    let (openapi, mut bindings, surface) = fixture();
    *request_field_mut(&mut bindings, "title") = "bool".into();

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["revise_job"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "capability.request_model_not_structurally_provable"
    );
}

#[test]
fn preserves_optional_nullable_request_depth() {
    let (mut openapi, mut bindings, surface) = fixture();
    *openapi
        .0
        .pointer_mut("/components/schemas/UpdateJobRequest/properties/priority")
        .expect("priority schema") = serde_json::json!({
        "anyOf": [
            {"type": "integer"},
            {"type": "null"}
        ]
    });
    *request_field_mut(&mut bindings, "priority") = "Option<Option<i64>>".into();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["revise_job"];
    assert_eq!(outcome.status, DerivationStatus::Derived);

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");
    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("pub fn priority(mut self, priority: i64) -> Self"));
    assert!(types.contains("self.raw.priority = Some(Some(priority));"));
    assert!(types.contains("pub fn priority_null(mut self) -> Self"));
    assert!(types.contains("self.raw.priority = Some(None);"));
}

#[test]
fn rejects_optional_nullable_request_with_shallow_raw_option() {
    let (mut openapi, bindings, surface) = fixture();
    *openapi
        .0
        .pointer_mut("/components/schemas/UpdateJobRequest/properties/priority")
        .expect("priority schema") = serde_json::json!({
        "anyOf": [
            {"type": "integer"},
            {"type": "null"}
        ]
    });

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["revise_job"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "capability.request_model_not_structurally_provable"
    );
}

#[test]
fn preserves_required_nullable_request_as_owned_raw_view() {
    let (mut openapi, mut bindings, surface) = fixture();
    *openapi
        .0
        .pointer_mut("/components/schemas/UpdateJobRequest/properties/title")
        .expect("title schema") = serde_json::json!({
        "anyOf": [
            {"type": "string"},
            {"type": "null"}
        ]
    });
    *request_field_mut(&mut bindings, "title") = "Option<String>".into();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["revise_job"];
    assert_eq!(outcome.status, DerivationStatus::Derived);

    let request = &derivation.definition.models["UpdateWorkJobsRequest"];
    assert_eq!(request.raw.as_deref(), Some("UpdateJobRequest"));
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
    .expect("derived raw-view request generates");
    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("pub struct UpdateWorkJobsRequest { raw: UpdateJobRequest }"));
    assert!(types.contains("pub fn into_raw(self) -> UpdateJobRequest"));
    let request_impl = types
        .split("impl UpdateWorkJobsRequest {")
        .nth(1)
        .expect("request implementation");
    assert!(
        !request_impl
            .split("impl From<UpdateJobRequest> for UpdateWorkJobsRequest")
            .next()
            .expect("request implementation end")
            .contains("pub fn new(")
    );
}

#[test]
fn derives_single_all_of_request_property_with_annotations() {
    let (mut openapi, bindings, surface) = fixture();
    *openapi
        .0
        .pointer_mut("/components/schemas/UpdateJobRequest/properties/priority")
        .expect("priority schema") = serde_json::json!({
        "allOf": [{"type": "integer"}],
        "default": 0,
        "title": "Priority"
    });

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface: surface.clone(),
        overrides: SdkOverrides::default(),
    })
    .expect("derive single allOf property");
    assert_eq!(
        derivation.report.operations["revise_job"].status,
        DerivationStatus::Derived
    );
    generate(GenerateInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("generate single allOf property");

    openapi.0["components"]["schemas"]["UpdateJobRequest"]["properties"]["priority"]["minimum"] =
        serde_json::json!(0);
    let rejected = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("fail closed on unmodeled sibling constraint");
    assert_eq!(
        rejected.report.operations["revise_job"].status,
        DerivationStatus::Rejected
    );
}

#[test]
fn preserves_root_flattened_request_as_owned_raw_view() {
    let (mut openapi, mut bindings, surface) = fixture();
    openapi.0["components"]["schemas"]["UpdateJobRequest"]["additionalProperties"] =
        serde_json::json!(true);
    bindings
        .structs
        .get_mut("UpdateJobRequest")
        .expect("request binding")
        .push(
            serde_json::from_value(serde_json::json!({
                "name": "additional_properties",
                "type": "std::collections::BTreeMap<String, serde_json::Value>"
            }))
            .expect("flattened binding field"),
        );

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive flattened root request");

    assert_eq!(
        derivation.report.operations["revise_job"].status,
        DerivationStatus::Derived
    );
    let request = &derivation.definition.models["UpdateWorkJobsRequest"];
    assert_eq!(request.raw.as_deref(), Some("UpdateJobRequest"));
    assert!(request.constructor.is_none());
    assert!(
        request
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
    .expect("flattened raw-view request generates");
    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("pub struct UpdateWorkJobsRequest { raw: UpdateJobRequest }"));
    assert!(types.contains("pub fn into_raw(self) -> UpdateJobRequest"));
    let request_impl = types
        .split("impl UpdateWorkJobsRequest {")
        .nth(1)
        .expect("request implementation");
    assert!(
        !request_impl
            .split("impl From<UpdateJobRequest> for UpdateWorkJobsRequest")
            .next()
            .expect("request implementation end")
            .contains("pub fn new(")
    );

    openapi.0["components"]["schemas"]["UpdateJobRequest"]["additionalProperties"] =
        serde_json::json!(false);
    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition,
        runtime: Runtime::default(),
    })
    .expect_err("flattened request drift must fail lowering");
    assert_eq!(error.diagnostic.code, "lower.request_drift");
}

