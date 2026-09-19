use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-array/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-array/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-inline-array/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn derives_inline_array_alias_without_schema_name_identity() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["read_inline_tags"];
    assert_eq!(outcome.status, DerivationStatus::Derived);
    assert_eq!(outcome.reason.code, "inference.structurally_proven");
    assert_eq!(outcome.binding.as_deref(), Some("call_list_13"));

    let response = &derivation.definition.models["TagsReportsResponse"];
    assert_eq!(response.raw.as_deref(), Some("OpaqueList7"));
    assert_eq!(response.type_alias, Some(true));

    let operation = &derivation.definition.resources["reports"].operations["tags"];
    assert_eq!(operation.raw_method.as_deref(), Some("call_list_13"));
    assert_eq!(operation.response.as_deref(), Some("TagsReportsResponse"));

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("derived definition generates");
    assert_eq!(generated.inventory.client, "ReportsClient");
    assert_eq!(generated.inventory.models, vec!["TagsReportsResponse"]);
    assert_eq!(generated.inventory.resources[0].path, vec!["reports"]);
    assert_eq!(generated.inventory.resources[0].operations, vec!["tags"]);

    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("pub type TagsReportsResponse = Vec<String>;"));
}

#[test]
fn rejects_inline_array_alias_drift_during_reconciliation() {
    let (openapi, mut bindings, surface) = fixture();
    bindings
        .aliases
        .insert("OpaqueList7".into(), "Vec<i64>".into());

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["read_inline_tags"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(outcome.reason.code, "bindings.no_structural_match");
}

#[test]
fn rejects_ambiguous_inline_array_bindings_without_name_identity() {
    let (openapi, mut bindings, surface) = fixture();
    let mut duplicate = bindings.operations["call_list_13"].clone();
    duplicate.name = "read_inline_tags".into();
    bindings
        .operations
        .insert("looks_like_operation_id".into(), duplicate);

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let outcome = &derivation.report.operations["read_inline_tags"];
    assert_eq!(outcome.status, DerivationStatus::Rejected);
    assert_eq!(
        outcome.reason.code,
        "bindings.source_operation_identity_required"
    );
}

#[test]
fn wraps_referenced_array_items_without_leaking_generated_rust_symbols() {
    let (mut openapi, mut bindings, surface) = fixture();
    openapi.0["components"] = serde_json::json!({
        "schemas": {
            "Record": {
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"]
            }
        }
    });
    openapi.0["paths"]["/reports/tags"]["get"]["responses"]["200"]["content"]["application/json"]
        ["schema"]["items"] = serde_json::json!({"$ref": "#/components/schemas/Record"});
    bindings
        .aliases
        .insert("OpaqueList7".into(), "Vec<Record>".into());
    bindings.structs.insert(
        "Record".into(),
        vec![rust_sdk_generator::FieldBinding {
            name: "name".into(),
            wire_name: Some("name".into()),
            type_name: "String".into(),
        }],
    );
    bindings
        .symbol_paths
        .insert("Record".into(), "crate::generated::types::Record".into());

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive referenced array view");
    assert_eq!(
        derivation.report.operations["read_inline_tags"].status,
        DerivationStatus::Derived
    );
    let response = &derivation.definition.models["TagsReportsResponse"];
    assert_eq!(response.type_alias, None);
    assert_eq!(response.borrowed, Some(false));
    assert!(
        response
            .accessors
            .as_ref()
            .expect("iter accessor")
            .contains_key("iter")
    );
    let item = &derivation.definition.models["TagsReportsResponseItem"];
    assert_eq!(item.raw.as_deref(), Some("Record"));
    assert_eq!(item.borrowed, Some(true));

    let generated = generate(GenerateInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        definition: derivation.definition.clone(),
        runtime: Runtime::default(),
    })
    .expect("referenced array view lowers");
    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("pub struct TagsReportsResponse { raw: OpaqueList7 }"));
    assert!(types.contains("pub struct TagsReportsResponseItem<'a>"));
    assert!(!types.contains("pub type TagsReportsResponse = Vec<Record>;"));

    openapi.0["paths"]["/reports/tags"]["get"]["responses"]["200"]["content"]["application/json"]
        ["schema"]["items"] = serde_json::json!({"$ref": "#/components/schemas/OtherRecord"});
    assert!(
        generate(GenerateInput {
            openapi,
            bindings,
            definition: derivation.definition,
            runtime: Runtime::default(),
        })
        .is_err(),
        "lowering must reject a drifted referenced item"
    );
}

#[test]
fn preserves_inline_array_of_named_union_as_owned_raw_view() {
    let (mut openapi, mut bindings, surface) = fixture();
    openapi.0["components"] = serde_json::json!({
        "schemas": {
            "Alpha": {"type": "object", "properties": {}},
            "Beta": {"type": "object", "properties": {}}
        }
    });
    openapi.0["paths"]["/reports/tags"]["get"]["responses"]["200"]["content"]["application/json"]
        ["schema"]["items"] = serde_json::json!({
        "anyOf": [
            {"$ref": "#/components/schemas/Alpha"},
            {"$ref": "#/components/schemas/Beta"}
        ]
    });
    bindings
        .aliases
        .insert("OpaqueList7".into(), "Vec<OpaqueItemUnion>".into());
    bindings.enums.insert(
        "OpaqueItemUnion".into(),
        vec![
            rust_sdk_generator::VariantBinding {
                name: "Alpha".into(),
                payload: Some("Alpha".into()),
                wire_name: None,
            },
            rust_sdk_generator::VariantBinding {
                name: "Beta".into(),
                payload: Some("Beta".into()),
                wire_name: None,
            },
        ],
    );
    bindings.structs.insert("Alpha".into(), Vec::new());
    bindings.structs.insert("Beta".into(), Vec::new());
    for name in ["OpaqueItemUnion", "Alpha", "Beta"] {
        bindings
            .symbol_paths
            .insert(name.into(), format!("crate::generated::types::{name}"));
    }

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive inline array union");

    assert_eq!(
        derivation.report.operations["read_inline_tags"].status,
        DerivationStatus::Derived
    );
    let response = &derivation.definition.models["TagsReportsResponse"];
    assert_eq!(response.raw.as_deref(), Some("OpaqueList7"));
    assert_eq!(response.type_alias, None);
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
    .expect("inline array union raw view generates");
    let types = &generated.files["facade_types.rs"];
    assert!(types.contains("pub struct TagsReportsResponse { raw: OpaqueList7 }"));
    assert!(!types.contains("pub type TagsReportsResponse = Vec<OpaqueItemUnion>;"));

    openapi.0["paths"]["/reports/tags"]["get"]["responses"]["200"]["content"]["application/json"]
        ["schema"]["items"]["anyOf"][1] = serde_json::json!({"$ref": "#/components/schemas/Alpha"});
    let error = generate(GenerateInput {
        openapi,
        bindings,
        definition,
        runtime: Runtime::default(),
    })
    .expect_err("array union drift must fail lowering");
    assert_eq!(error.diagnostic.code, "lower.response_drift");
}
