use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use rust_sdk_generator::{
    ApiInventory, Derivation, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture() -> GenerateInput {
    GenerateInput {
        openapi: OpenApi(
            serde_json::from_str(include_str!("fixtures/library/openapi.json"))
                .expect("fixture OpenAPI"),
        ),
        bindings: serde_json::from_str(include_str!("fixtures/library/rust-bindings.json"))
            .expect("fixture bindings"),
        definition: serde_json::from_str(include_str!("fixtures/library/policy.json"))
            .expect("fixture definition"),
        runtime: Runtime::default(),
    }
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/library")
        .join(name)
}

fn derivation_fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/derivation-response")
        .join(name)
}

fn temp_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "rust-sdk-generator-surface-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn cli_and_library_generate_identical_sdk_and_inventory() {
    let expected = generate(fixture()).expect("library generation");
    let output_dir = temp_dir();
    let result = Command::new(env!("CARGO_BIN_EXE_rust-sdk-generator"))
        .args([
            "generate",
            "--openapi",
            fixture_path("openapi.json").to_str().expect("utf8 path"),
            "--bindings",
            fixture_path("rust-bindings.json")
                .to_str()
                .expect("utf8 path"),
            "--definition",
            fixture_path("policy.json").to_str().expect("utf8 path"),
            "--output",
            output_dir.to_str().expect("utf8 path"),
        ])
        .output()
        .expect("run CLI");
    assert!(
        result.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let inventory: ApiInventory =
        serde_json::from_slice(&result.stdout).expect("machine-readable inventory");
    assert_eq!(inventory, expected.inventory);
    for (name, source) in &expected.files {
        assert_eq!(
            fs::read_to_string(output_dir.join(name)).expect("generated file"),
            *source,
            "CLI/library output drift for {name}"
        );
    }

    let check = Command::new(env!("CARGO_BIN_EXE_rust-sdk-generator"))
        .args([
            "check",
            "--openapi",
            fixture_path("openapi.json").to_str().expect("utf8 path"),
            "--bindings",
            fixture_path("rust-bindings.json")
                .to_str()
                .expect("utf8 path"),
            "--definition",
            fixture_path("policy.json").to_str().expect("utf8 path"),
        ])
        .output()
        .expect("run check CLI");
    assert!(check.status.success());
    let checked: ApiInventory = serde_json::from_slice(&check.stdout).expect("check inventory");
    assert_eq!(checked, expected.inventory);

    fs::remove_dir_all(output_dir).expect("cleanup output");
}

#[test]
fn cli_and_library_derive_identical_contracts() {
    let input = fixture();
    let expected = derive(DeriveInput {
        openapi: input.openapi,
        bindings: input.bindings,
        surface: PublicSdkSurface::default(),
        overrides: SdkOverrides::default(),
    })
    .expect("library derivation");

    let result = Command::new(env!("CARGO_BIN_EXE_rust-sdk-generator"))
        .args([
            "derive",
            "--openapi",
            fixture_path("openapi.json").to_str().expect("utf8 path"),
            "--bindings",
            fixture_path("rust-bindings.json")
                .to_str()
                .expect("utf8 path"),
        ])
        .output()
        .expect("run derive CLI");
    assert!(
        result.status.success(),
        "derive CLI failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let actual: Derivation =
        serde_json::from_slice(&result.stdout).expect("machine-readable derivation");
    assert_eq!(actual, expected);
}

#[test]
fn cli_derive_output_is_directly_consumable_by_generate() {
    let openapi_path = derivation_fixture_path("openapi.json");
    let bindings_path = derivation_fixture_path("rust-bindings.json");
    let surface_path = derivation_fixture_path("surface.json");

    let openapi: OpenApi =
        serde_json::from_str(&fs::read_to_string(&openapi_path).expect("OpenAPI fixture"))
            .expect("OpenAPI JSON");
    let bindings: rust_sdk_generator::Bindings =
        serde_json::from_str(&fs::read_to_string(&bindings_path).expect("bindings fixture"))
            .expect("bindings JSON");
    let surface: PublicSdkSurface =
        serde_json::from_str(&fs::read_to_string(&surface_path).expect("surface fixture"))
            .expect("surface JSON");

    let expected_derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    })
    .expect("library derive");
    let expected = generate(GenerateInput {
        openapi,
        bindings,
        definition: expected_derivation.definition.clone(),
        runtime: Runtime::default(),
    })
    .expect("library generate from derived definition");

    let derive_result = Command::new(env!("CARGO_BIN_EXE_rust-sdk-generator"))
        .args([
            "derive",
            "--openapi",
            openapi_path.to_str().expect("utf8 path"),
            "--bindings",
            bindings_path.to_str().expect("utf8 path"),
            "--surface",
            surface_path.to_str().expect("utf8 path"),
        ])
        .output()
        .expect("run derive CLI");
    assert!(
        derive_result.status.success(),
        "derive CLI failed: {}",
        String::from_utf8_lossy(&derive_result.stderr)
    );
    let derived: Derivation =
        serde_json::from_slice(&derive_result.stdout).expect("derive contract JSON");
    assert_eq!(derived, expected_derivation);

    let working = temp_dir();
    fs::create_dir_all(&working).expect("create working directory");
    let definition_path = working.join("sdk-definition.json");
    fs::write(
        &definition_path,
        serde_json::to_vec_pretty(&derived.definition).expect("definition JSON"),
    )
    .expect("write definition");
    let output_dir = working.join("sdk");

    let generate_result = Command::new(env!("CARGO_BIN_EXE_rust-sdk-generator"))
        .args([
            "generate",
            "--openapi",
            openapi_path.to_str().expect("utf8 path"),
            "--bindings",
            bindings_path.to_str().expect("utf8 path"),
            "--definition",
            definition_path.to_str().expect("utf8 path"),
            "--output",
            output_dir.to_str().expect("utf8 path"),
        ])
        .output()
        .expect("run generate CLI");
    assert!(
        generate_result.status.success(),
        "generate CLI failed: {}",
        String::from_utf8_lossy(&generate_result.stderr)
    );

    let inventory: ApiInventory =
        serde_json::from_slice(&generate_result.stdout).expect("generated inventory");
    assert_eq!(inventory, expected.inventory);
    for (name, source) in &expected.files {
        assert_eq!(
            fs::read_to_string(output_dir.join(name)).expect("generated source"),
            *source,
            "derive -> generate CLI/library drift for {name}"
        );
    }

    let check_result = Command::new(env!("CARGO_BIN_EXE_rust-sdk-generator"))
        .args([
            "check",
            "--openapi",
            openapi_path.to_str().expect("utf8 path"),
            "--bindings",
            bindings_path.to_str().expect("utf8 path"),
            "--definition",
            definition_path.to_str().expect("utf8 path"),
        ])
        .output()
        .expect("run check CLI");
    assert!(
        check_result.status.success(),
        "check CLI failed: {}",
        String::from_utf8_lossy(&check_result.stderr)
    );
    let checked: ApiInventory =
        serde_json::from_slice(&check_result.stdout).expect("checked inventory");
    assert_eq!(checked, expected.inventory);

    fs::remove_dir_all(working).expect("cleanup working directory");
}
