use openapi_to_rust_bindings::Bindings;
use serde_json::{Value, json};

fn fixture() -> Bindings {
    Bindings::from_value(json!({
        "schema_version": 4,
        "structs": {},
        "enums": {},
        "aliases": {},
        "operations": {},
        "symbol_paths": {},
        "binding": {
            "client": {
                "type_path": "crate::generated::client::HttpClient",
                "constructor": "new",
                "api_key_builder": "with_api_key",
                "base_url_builder": "with_base_url"
            },
            "type_preludes": ["crate::generated::types::*"]
        }
    }))
    .expect("validate Bindings v4 fixture")
}

#[test]
fn canonical_bindings_v4_fixture_is_accepted() {
    assert_eq!(fixture().as_value()["schema_version"], 4);
}

#[test]
fn validation_rejects_unknown_fields_and_duplicate_preludes() {
    let mut unknown = fixture().to_value();
    unknown
        .as_object_mut()
        .expect("bindings object")
        .insert("unexpected".to_owned(), Value::Null);
    assert!(Bindings::from_value(unknown).is_err());

    let mut duplicate = fixture().to_value();
    duplicate["binding"]["type_preludes"] =
        json!(["crate::generated::types::*", "crate::generated::types::*"]);
    assert!(Bindings::from_value(duplicate).is_err());
}

#[test]
fn shim_rejects_legacy_bindings_versions() {
    let mut legacy = fixture().to_value();
    legacy["schema_version"] = Value::from(3);
    assert!(Bindings::from_value(legacy).is_err());
}
