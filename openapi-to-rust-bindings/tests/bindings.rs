use openapi_to_rust_bindings::{
    Bindings, MANIFEST_NAME, parse_binding_manifest, parse_bindings, read_bindings,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "openapi-to-rust-bindings-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn expected(name: &str) -> Bindings {
    let value: Value = serde_json::from_str(
        &fs::read_to_string(fixtures().join(name).join("rust-bindings.json"))
            .expect("read fixture bindings"),
    )
    .expect("parse fixture bindings");
    Bindings::from_value(value).expect("validate fixture bindings")
}

fn historical_common_contract(bindings: &Bindings) -> Value {
    let mut value = bindings.to_value();
    value["schema_version"] = Value::from(2);
    if let Some(structs) = value.get_mut("structs").and_then(Value::as_object_mut) {
        for fields in structs.values_mut().filter_map(Value::as_array_mut) {
            for field in fields.iter_mut().filter_map(Value::as_object_mut) {
                field.remove("wire_name");
            }
        }
    }
    if let Some(operations) = value.get_mut("operations").and_then(Value::as_object_mut) {
        for operation in operations.values_mut().filter_map(Value::as_object_mut) {
            operation.remove("metadata");
        }
    }
    value
}

#[test]
fn legacy_fixtures_match_checked_in_bindings_contract() {
    for name in ["menagerie", "library"] {
        let root = fixtures().join(name);
        let actual = parse_bindings(
            &fs::read_to_string(root.join("types.rs")).expect("read types fixture"),
            &fs::read_to_string(root.join("client.rs")).expect("read client fixture"),
        )
        .expect("parse generated bindings");
        assert_eq!(actual, expected(name));
    }
}

#[test]
fn menagerie_manifest_preserves_the_historical_common_contract() {
    let root = fixtures().join("menagerie");
    let manifest = parse_binding_manifest(
        &fs::read_to_string(root.join(MANIFEST_NAME)).expect("read manifest fixture"),
    )
    .expect("normalize manifest");
    assert_eq!(manifest.as_value()["schema_version"], Value::from(3));
    assert_eq!(
        historical_common_contract(&manifest),
        expected("menagerie").to_value()
    );

    let read = read_bindings(&root).expect("prefer manifest metadata");
    assert_eq!(read, manifest);
    assert_eq!(
        read.as_value()["operations"]["adopt"]["metadata"]["source_operation"],
        serde_json::json!({
            "operation_id": "adopt",
            "method": "POST",
            "path": "/animals"
        })
    );
    assert_eq!(
        read.as_value()["operations"]["adopt"]["metadata"]["representation"]["kind"],
        "json"
    );
}

#[test]
fn library_legacy_sidecar_remains_first_class_during_migration() {
    let root = fixtures().join("library");
    assert_eq!(
        read_bindings(&root).expect("read library sidecar"),
        expected("library")
    );
}

#[test]
fn generated_serde_rename_is_preserved() {
    let bindings = parse_bindings(
        r#"
pub enum State {
    #[serde(rename = "in-progress")]
    InProgress,
}
"#,
        "",
    )
    .expect("parse enum");
    assert_eq!(
        bindings.as_value()["enums"]["State"][0]["wire_name"],
        Value::String("in-progress".to_owned())
    );
}

#[test]
fn sidecar_does_not_require_generated_sources() {
    let root = TestDir::new();
    fs::copy(
        fixtures().join("menagerie/rust-bindings.json"),
        root.path().join("rust-bindings.json"),
    )
    .expect("copy sidecar");
    assert_eq!(
        read_bindings(root.path()).expect("read sidecar"),
        expected("menagerie")
    );
}

#[test]
fn invalid_manifest_fails_closed_without_sidecar_or_source_fallback() {
    let root = TestDir::new();
    let fixture = fixtures().join("menagerie");
    fs::copy(
        fixture.join("rust-bindings.json"),
        root.path().join("rust-bindings.json"),
    )
    .expect("copy sidecar");
    fs::copy(fixture.join("types.rs"), root.path().join("types.rs")).expect("copy types");
    fs::copy(fixture.join("client.rs"), root.path().join("client.rs")).expect("copy client");
    fs::write(root.path().join(MANIFEST_NAME), "{not json").expect("write manifest");

    let error = read_bindings(root.path()).expect_err("invalid manifest must fail");
    assert!(error.to_string().starts_with("invalid binding-manifest.json:"));
}

#[test]
fn manifest_schema_identifier_version_and_required_fields_fail_closed() {
    let source = fs::read_to_string(fixtures().join("menagerie").join(MANIFEST_NAME))
        .expect("read manifest");
    let value: Value = serde_json::from_str(&source).expect("parse fixture");

    for (path, replacement, message) in [
        ("/schema", Value::String("other.schema".into()), "schema identifier"),
        ("/schema_version", Value::from(99), "schema version"),
    ] {
        let mut invalid = value.clone();
        *invalid.pointer_mut(path).expect("manifest path") = replacement;
        let error =
            parse_binding_manifest(&serde_json::to_string(&invalid).expect("serialize manifest"))
                .expect_err("unsupported manifest contract must fail");
        assert!(error.to_string().contains(message));
    }

    let mut missing = value.clone();
    missing["structs"]["AnimalRequest"][0]
        .as_object_mut()
        .expect("field")
        .remove("wire_name");
    let error = parse_binding_manifest(
        &serde_json::to_string(&missing).expect("serialize missing field manifest"),
    )
    .expect_err("missing nullable field must still fail");
    assert!(error.to_string().contains("wire_name"));
}

#[test]
fn invalid_sidecar_fails_closed() {
    let root = TestDir::new();
    fs::write(root.path().join("rust-bindings.json"), "{not json").expect("write sidecar");
    let error = read_bindings(root.path()).expect_err("invalid sidecar must fail");
    assert_eq!(error.to_string(), "invalid rust-bindings.json");
}

#[test]
fn schema_invalid_sidecar_fails_closed() {
    let root = TestDir::new();
    let mut value = expected("menagerie").to_value();
    value
        .as_object_mut()
        .expect("bindings object")
        .insert("unexpected".to_owned(), Value::Bool(true));
    fs::write(
        root.path().join("rust-bindings.json"),
        serde_json::to_vec(&value).expect("serialize invalid sidecar"),
    )
    .expect("write invalid sidecar");
    let error = read_bindings(root.path()).expect_err("schema-invalid sidecar must fail");
    assert_eq!(error.to_string(), "invalid rust-bindings.json");
}

#[test]
fn legacy_generated_sources_remain_supported() {
    let root = TestDir::new();
    let fixture = fixtures().join("library");
    fs::copy(fixture.join("types.rs"), root.path().join("types.rs")).expect("copy types");
    fs::copy(fixture.join("client.rs"), root.path().join("client.rs")).expect("copy client");
    assert_eq!(
        read_bindings(root.path()).expect("read legacy generated sources"),
        expected("library")
    );
}

#[test]
fn bindings_validation_rejects_unknown_fields_and_duplicate_preludes() {
    let mut unknown = expected("menagerie").to_value();
    unknown
        .as_object_mut()
        .expect("bindings object")
        .insert("unexpected".to_owned(), Value::Null);
    assert!(Bindings::from_value(unknown).is_err());

    let mut duplicate = expected("menagerie").to_value();
    duplicate["binding"]["type_preludes"] =
        serde_json::json!(["crate::generated::types::*", "crate::generated::types::*"]);
    assert!(Bindings::from_value(duplicate).is_err());
}
