use rust_sdk_generator::{
    Bindings, DerivationStatus, DeriveInput, GenerateInput, OpenApi, PublicSdkSurface, Runtime,
    SdkOverrides, derive, generate,
};

fn fixture() -> (OpenApi, Bindings, PublicSdkSurface) {
    let openapi = OpenApi(serde_json::json!({
        "openapi": "3.1.0",
        "paths": {"/audio": {"post": {
            "operationId": "synthesize",
            "requestBody": {"required": true, "content": {"application/json": {
                "schema": {"$ref": "#/components/schemas/SynthRequest"}
            }}},
            "responses": {"204": {"description": "done"}}
        }}},
        "components": {"schemas": {"SynthRequest": {
            "type": "object",
            "additionalProperties": false,
            "required": ["input"],
            "properties": {
                "input": {"type": "string"},
                "audio": {"anyOf": [
                    {"type": "string"},
                    {"type": "string"},
                    {"type": "null"}
                ]}
            }
        }}}
    }));
    let bindings: Bindings = serde_json::from_value(serde_json::json!({
        "schema_version": 2,
        "structs": {"SynthRequest": [
            {"name": "input", "type": "String"},
            {"name": "audio", "type": "Option<Option<AudioRef>>"}
        ]},
        "enums": {},
        "aliases": {"AudioRef": "String"},
        "operations": {"raw_synthesize": {
            "name": "raw_synthesize",
            "parameters": [{"name": "request", "type": "SynthRequest"}],
            "return_type": "Result<(), Error>",
            "success_type": "()"
        }},
        "symbol_paths": {
            "SynthRequest": "crate::generated::types::SynthRequest",
            "AudioRef": "crate::generated::types::AudioRef"
        },
        "binding": {
            "client": {
                "type_path": "crate::generated::client::HttpClient",
                "constructor": "new",
                "api_key_builder": "with_api_key",
                "base_url_builder": "with_base_url"
            },
            "type_preludes": ["crate::generated::types::*"]
        }
    })).expect("bindings fixture");
    let surface = PublicSdkSurface {
        schema_version: 1,
        client: Some("AudioClient".into()),
        operations: std::collections::BTreeMap::from([(
            "synthesize".into(),
            vec!["audio.synthesize".into()],
        )]),
    };
    (openapi, bindings, surface)
}

#[test]
fn derives_redundant_anyof_string_alias_without_losing_nullability() {
    let (openapi, bindings, surface) = fixture();
    let derivation = derive(DeriveInput {
        openapi: openapi.clone(),
        bindings: bindings.clone(),
        surface,
        overrides: SdkOverrides::default(),
    }).expect("derive redundant anyOf request");

    assert_eq!(
        derivation.report.operations["synthesize"].status,
        DerivationStatus::Derived
    );
    let request = &derivation.definition.models["SynthesizeAudioRequest"];
    assert_eq!(request.raw.as_deref(), Some("SynthRequest"));
    assert_eq!(request.constructor.as_deref(), Some(&["input".to_owned()][..]));

    generate(GenerateInput {
        openapi,
        bindings,
        definition: derivation.definition,
        runtime: Runtime::default(),
    }).expect("generate exact request and empty response");
}

#[test]
fn mixed_or_exclusive_or_constrained_unions_are_not_collapsed() {
    let (openapi, bindings, surface) = fixture();
    let base = openapi.0["components"]["schemas"]["SynthRequest"]["properties"]["audio"].clone();
    for changed in [
        serde_json::json!({"oneOf": [
            {"type": "string"}, {"type": "string"}, {"type": "null"}
        ]}),
        serde_json::json!({"anyOf": [
            {"type": "string"}, {"type": "integer"}, {"type": "null"}
        ]}),
        serde_json::json!({
            "anyOf": [
                {"type": "string"}, {"type": "string"}, {"type": "null"}
            ],
            "minLength": 2
        }),
    ] {
        let mut changed_openapi = openapi.clone();
        changed_openapi.0["components"]["schemas"]["SynthRequest"]["properties"]["audio"] =
            changed;
        let derivation = derive(DeriveInput {
            openapi: changed_openapi,
            bindings: bindings.clone(),
            surface: surface.clone(),
            overrides: SdkOverrides::default(),
        }).expect("unsupported shape is classified");
        assert_eq!(
            derivation.report.operations["synthesize"].status,
            DerivationStatus::Rejected
        );
    }
    assert_ne!(base, serde_json::Value::Null);
}
