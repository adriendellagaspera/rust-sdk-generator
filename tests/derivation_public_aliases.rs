use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = serde_json::from_str(include_str!(
        "fixtures/derivation-public-aliases/openapi.json"
    ))
    .expect("fixture OpenAPI");
    let bindings = serde_json::from_str(include_str!(
        "fixtures/derivation-public-aliases/rust-bindings.json"
    ))
    .expect("fixture bindings");
    let surface = serde_json::from_str(include_str!(
        "fixtures/derivation-public-aliases/surface.json"
    ))
    .expect("fixture surface");
    (openapi, bindings, surface)
}

#[test]
fn projects_all_explicit_public_aliases_once_per_source_operation() {
    let (openapi, bindings, surface) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    let alpha = &derivation.report.operations["read_alpha"];
    assert_eq!(alpha.status, DerivationStatus::Derived);
    assert_eq!(
        alpha.public_paths,
        vec!["legacy.alpha".to_owned(), "things.alpha".to_owned()]
    );
    assert_eq!(alpha.public_path.as_deref(), Some("legacy.alpha"));
    assert_eq!(alpha.binding.as_deref(), Some("opaque_alpha_7"));

    assert_eq!(
        derivation.definition.resources["legacy"].operations["alpha"].operation_id,
        "read_alpha"
    );
    assert_eq!(
        derivation.definition.resources["things"].operations["alpha"].operation_id,
        "read_alpha"
    );
    assert_eq!(
        derivation.definition.resources["things"].operations["beta"].operation_id,
        "read_beta"
    );

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("generate");

    let mut operation_count = 0;
    for resource in &generated.inventory.resources {
        operation_count += resource.operations.len();
    }
    assert_eq!(operation_count, 3);
    assert!(generated.files["legacy.rs"].contains("pub async fn alpha(&self)"));
    assert!(generated.files["things.rs"].contains("pub async fn alpha(&self)"));
    assert!(generated.files["things.rs"].contains("pub async fn beta(&self)"));
}

#[test]
fn missing_surface_uses_deterministic_route_fallback_end_to_end() {
    let (openapi, bindings, _) = fixture();

    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface: PublicSdkSurface::default(),
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    assert_eq!(
        derivation.report.operations["read_alpha"].public_path.as_deref(),
        Some("things.a.read_alpha")
    );
    assert_eq!(
        derivation.report.operations["read_beta"].public_path.as_deref(),
        Some("things.b.read_beta")
    );

    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    })
    .expect("generate");
    assert_eq!(generated.inventory.client, "Client");
    assert!(generated.files.contains_key("things_a.rs"));
    assert!(generated.files.contains_key("things_b.rs"));
}

#[test]
fn colliding_aliases_reject_both_source_operations_without_partial_projection() {
    let (openapi, bindings, mut surface) = fixture();
    surface
        .operations
        .insert("read_alpha".into(), vec!["things.shared".into()]);
    surface
        .operations
        .insert("read_beta".into(), vec!["things.shared".into()]);

    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("derive");

    for operation_id in ["read_alpha", "read_beta"] {
        let outcome = &derivation.report.operations[operation_id];
        assert_eq!(outcome.status, DerivationStatus::Rejected);
        assert_eq!(outcome.reason.code, "surface.public_path_collision");
    }
    assert!(derivation.definition.resources.is_empty());
}
