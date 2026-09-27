use openapi_to_rust_bindings::Bindings;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture(name: &str) -> Bindings {
    let value: Value = serde_json::from_str(
        &fs::read_to_string(fixtures().join(name).join("rust-bindings.json"))
            .expect("read fixture bindings"),
    )
    .expect("parse fixture bindings");
    Bindings::from_value(value).expect("validate fixture bindings")
}

#[test]
fn canonical_bindings_fixture_is_accepted() {
    let bindings = fixture("library");
    assert_eq!(bindings.as_value()["schema_version"], 2);
}

#[test]
fn validation_rejects_unknown_fields_and_duplicate_preludes() {
    let mut unknown = fixture("library").to_value();
    unknown
        .as_object_mut()
        .expect("bindings object")
        .insert("unexpected".to_owned(), Value::Null);
    assert!(Bindings::from_value(unknown).is_err());

    let mut duplicate = fixture("library").to_value();
    duplicate["binding"]["type_preludes"] =
        serde_json::json!(["crate::generated::types::*", "crate::generated::types::*"]);
    assert!(Bindings::from_value(duplicate).is_err());
}
