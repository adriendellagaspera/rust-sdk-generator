use openapi_to_rust_bindings::{Bindings, MANIFEST_NAME, parse_binding_manifest, read_bindings};
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
fn manifest_fixtures_preserve_the_checked_in_common_contract() {
    for name in ["menagerie", "library"] {
        let root = fixtures().join(name);
        let manifest = parse_binding_manifest(
            &fs::read_to_string(root.join(MANIFEST_NAME)).expect("read manifest fixture"),
        )
        .expect("normalize manifest");
        assert_eq!(manifest.as_value()["schema_version"], Value::from(3));
        assert_eq!(
            historical_common_contract(&manifest),
            expected(name).to_value(),
            "{name} manifest/common contract diverged from the checked-in canonical sidecar"
        );

        let read = read_bindings(&root).expect("prefer manifest metadata");
        assert_eq!(read, manifest);
    }

    let menagerie = read_bindings(fixtures().join("menagerie")).expect("read menagerie manifest");
    assert_eq!(
        menagerie.as_value()["operations"]["adopt"]["metadata"]["source_operation"],
        serde_json::json!({
            "operation_id": "adopt",
            "method": "POST",
            "path": "/animals"
        })
    );
    assert_eq!(
        menagerie.as_value()["operations"]["adopt"]["metadata"]["representation"]["kind"],
        "json"
    );
}

#[test]
fn canonical_sidecar_remains_supported() {
    let root = TestDir::new();
    fs::copy(
        fixtures().join("library/rust-bindings.json"),
        root.path().join("rust-bindings.json"),
    )
    .expect("copy sidecar");
    assert_eq!(
        read_bindings(root.path()).expect("read library sidecar"),
        expected("library")
    );
}

#[test]
fn manifest_path_ignores_generated_sources() {
    let root = TestDir::new();
    fs::copy(
        fixtures().join("transport").join(MANIFEST_NAME),
        root.path().join(MANIFEST_NAME),
    )
    .expect("copy transport manifest");
    fs::write(
        root.path().join("types.rs"),
        r#"
#[cfg(not(target_arch = "wasm32"))]
pub type HttpResponseByteStream =
    futures_util::stream::BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>;
#[cfg(target_arch = "wasm32")]
pub type HttpResponseByteStream =
    futures_util::stream::LocalBoxStream<'static, Result<bytes::Bytes, reqwest::Error>>;
"#,
    )
    .expect("write cfg-exclusive aliases");
    fs::write(root.path().join("client.rs"), "this is not Rust").expect("write client source");

    let bindings = read_bindings(root.path()).expect("manifest metadata must be authoritative");
    assert_eq!(
        bindings.as_value()["operations"]["render_stream_2"]["metadata"]["stream_abi"]["alias"],
        "HttpResponseByteStream"
    );
}

#[test]
fn transport_manifest_preserves_generator_owned_semantics() {
    let source = fs::read_to_string(fixtures().join("transport").join(MANIFEST_NAME))
        .expect("read transport manifest");
    let bindings = parse_binding_manifest(&source).expect("normalize transport manifest");
    let value = bindings.as_value();

    assert_eq!(value["schema_version"], 3);
    assert_eq!(value["structs"]["CreateRequest"][0]["name"], "r#type");
    assert_eq!(value["structs"]["CreateRequest"][0]["wire_name"], "type");
    assert_eq!(value["enums"]["Mode"][0]["wire_name"], "fast-mode");
    assert_eq!(value["enums"]["Mode"][1]["payload"], "String");
    assert_eq!(value["aliases"]["Identifier"], "String");
    assert_eq!(
        value["symbol_paths"]["Identifier"],
        "crate::generated::types::Identifier"
    );
    assert_eq!(
        value["binding"]["client"]["type_path"],
        "crate::generated::client::HttpClient"
    );
    assert_eq!(
        value["binding"]["type_preludes"][0],
        "crate::generated::types::*"
    );

    let render = &value["operations"]["render"];
    let stream = &value["operations"]["render_stream_2"];
    assert_eq!(
        render["metadata"]["source_operation"],
        stream["metadata"]["source_operation"]
    );
    assert_eq!(render["metadata"]["representation"]["kind"], "json");
    assert_eq!(stream["metadata"]["representation"]["kind"], "event_stream");
    assert_eq!(
        stream["metadata"]["request_discriminators"][0]["wire_name"],
        "stream"
    );
    assert_eq!(
        stream["metadata"]["stream_abi"]["native_type"],
        "futures_util::stream::BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>"
    );
    assert_eq!(
        stream["metadata"]["stream_abi"]["wasm_type"],
        "futures_util::stream::LocalBoxStream<'static, Result<bytes::Bytes, reqwest::Error>>"
    );
    assert_eq!(
        value["operations"]["download"]["metadata"]["representation"]["kind"],
        "binary_buffered"
    );
    assert_eq!(
        value["operations"]["download_live"]["metadata"]["representation"]["kind"],
        "binary_stream"
    );
    assert_eq!(
        value["operations"]["delete_item"]["metadata"]["representation"]["kind"],
        "empty"
    );
    assert_eq!(
        value["operations"]["read_text"]["metadata"]["success_statuses"],
        serde_json::json!([])
    );
    assert_eq!(
        value["operations"]["create_item_with_multipart_filenames"]["metadata"]["kind"],
        "multipart_filenames"
    );
}

#[test]
fn manifest_rejects_unknown_representation_and_canonical_collisions() {
    let source = fs::read_to_string(fixtures().join("transport").join(MANIFEST_NAME))
        .expect("read transport manifest");
    let value: Value = serde_json::from_str(&source).expect("parse manifest");

    let mut unknown = value.clone();
    unknown["operations"][0]["representation"]["kind"] = Value::String("telepathy".into());
    let error =
        parse_binding_manifest(&serde_json::to_string(&unknown).expect("serialize manifest"))
            .expect_err("unknown representation must fail");
    assert!(error.to_string().contains("schema decode failed"));

    let mut duplicate = value;
    let mut operation = duplicate["operations"][0].clone();
    operation["rust_method_name"] = Value::String("create_item_duplicate".into());
    duplicate["operations"]
        .as_array_mut()
        .expect("operations")
        .push(operation);
    let error =
        parse_binding_manifest(&serde_json::to_string(&duplicate).expect("serialize manifest"))
            .expect_err("canonical collision must fail");
    assert!(
        error
            .to_string()
            .contains("duplicate canonical source-operation/representation identity")
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
    assert!(
        error
            .to_string()
            .starts_with("invalid binding-manifest.json:")
    );
}

#[test]
fn manifest_schema_identifier_version_and_required_fields_fail_closed() {
    let source = fs::read_to_string(fixtures().join("menagerie").join(MANIFEST_NAME))
        .expect("read manifest");
    let value: Value = serde_json::from_str(&source).expect("parse fixture");

    for (path, replacement, message) in [
        (
            "/schema",
            Value::String("other.schema".into()),
            "schema identifier",
        ),
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
    assert!(
        error
            .to_string()
            .starts_with("invalid rust-bindings.json JSON:")
    );
    assert!(error.to_string().contains("line 1 column"));
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
    assert!(
        error
            .to_string()
            .starts_with("invalid rust-bindings.json: invalid Bindings:")
    );
    assert!(error.to_string().contains("unexpected"));
}

#[test]
fn generated_sources_without_metadata_are_rejected() {
    let root = TestDir::new();
    let fixture = fixtures().join("library");
    fs::copy(fixture.join("types.rs"), root.path().join("types.rs")).expect("copy types");
    fs::copy(fixture.join("client.rs"), root.path().join("client.rs")).expect("copy client");
    let error = read_bindings(root.path()).expect_err("generated Rust is not a bindings input");
    assert!(error.to_string().contains(MANIFEST_NAME));
    assert!(error.to_string().contains("rust-bindings.json"));
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
