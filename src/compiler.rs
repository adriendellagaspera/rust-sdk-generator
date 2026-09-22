use std::collections::BTreeMap;

use crate::contracts::{
    ApiInventory, Bindings, GenerateInput, GeneratedSdk, OpenApi, ResourceInventory, Runtime,
    SdkDefinition,
};
use crate::emit;
use crate::error::Result;
use crate::ir::FacadeIr;
use crate::lower;

pub(crate) fn compile(
    openapi: &OpenApi,
    bindings: &Bindings,
    definition: &SdkDefinition,
    runtime: &Runtime,
) -> Result<(FacadeIr, BTreeMap<String, String>)> {
    let ir = lower::lower(openapi, bindings, definition, runtime)?;
    let files = emit::emit(&ir, &bindings.binding, runtime)?;
    Ok((ir, files))
}

pub(crate) fn generate(input: GenerateInput) -> Result<GeneratedSdk> {
    let GenerateInput {
        openapi,
        bindings,
        definition,
        runtime,
    } = input;
    let (ir, files) = compile(&openapi, &bindings, &definition, &runtime)?;
    let inventory = ApiInventory {
        client: ir.client_name.clone(),
        models: ir.models.iter().map(|model| model.name.clone()).collect(),
        resources: ir
            .resources
            .iter()
            .map(|resource| ResourceInventory {
                path: resource.path.clone(),
                module: resource.module.clone(),
                name: resource.name.clone(),
                operations: resource
                    .operations
                    .iter()
                    .map(|operation| operation.name.clone())
                    .collect(),
            })
            .collect(),
    };
    Ok(GeneratedSdk { files, inventory })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::Value;
    use sha2::{Digest, Sha256};

    use super::*;

    fn fixture(name: &str) -> (OpenApi, Bindings, SdkDefinition) {
        let (openapi, bindings, definition) = match name {
            "menagerie" => (
                include_str!("../tests/fixtures/menagerie/openapi.json"),
                include_str!("../tests/fixtures/menagerie/rust-bindings.json"),
                include_str!("../tests/fixtures/menagerie/policy.json"),
            ),
            "library" => (
                include_str!("../tests/fixtures/library/openapi.json"),
                include_str!("../tests/fixtures/library/rust-bindings.json"),
                include_str!("../tests/fixtures/library/policy.json"),
            ),
            _ => panic!("unknown fixture {name}"),
        };
        (
            OpenApi(serde_json::from_str(openapi).expect("fixture OpenAPI")),
            serde_json::from_str(bindings).expect("fixture bindings"),
            serde_json::from_str(definition).expect("fixture definition"),
        )
    }

    fn sha256(value: &str) -> String {
        Sha256::digest(value.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[test]
    fn generic_fixtures_match_frozen_python_oracle_byte_for_byte() {
        let oracle: BTreeMap<String, BTreeMap<String, String>> =
            serde_json::from_str(include_str!("../tests/oracle/current-generator.json"))
                .expect("frozen oracle");

        for fixture_name in ["menagerie", "library"] {
            let (openapi, bindings, definition) = fixture(fixture_name);
            let (_, files) = compile(&openapi, &bindings, &definition, &Runtime::default())
                .unwrap_or_else(|error| panic!("{fixture_name}: {error:?}"));
            let expected = &oracle[fixture_name];
            assert_eq!(
                files.keys().collect::<Vec<_>>(),
                expected.keys().collect::<Vec<_>>(),
                "{fixture_name}: generated file set drift"
            );
            for (name, source) in files {
                let actual = sha256(&source);
                assert_eq!(
                    expected[&name], actual,
                    "{fixture_name}/{name} output drift:\n{source}"
                );
            }
        }
    }

    #[test]
    fn lowering_produces_stable_public_inventory_order() {
        let (openapi, bindings, definition) = fixture("library");
        let (ir, _) = compile(&openapi, &bindings, &definition, &Runtime::default())
            .expect("library compiles");
        assert_eq!(
            ir.models
                .iter()
                .map(|model| model.name.as_str())
                .collect::<Vec<_>>(),
            vec!["NewBook", "Book", "BookCollection"]
        );
        let books = ir
            .resources
            .iter()
            .find(|resource| resource.module == "catalog_books")
            .expect("books resource");
        assert_eq!(
            books
                .operations
                .iter()
                .map(|operation| operation.name.as_str())
                .collect::<Vec<_>>(),
            vec!["create", "list", "delete", "download"]
        );
    }

    #[test]
    fn compilation_is_deterministic() {
        let (openapi, bindings, definition) = fixture("menagerie");
        let first = compile(&openapi, &bindings, &definition, &Runtime::default())
            .expect("first compile")
            .1;
        let second = compile(&openapi, &bindings, &definition, &Runtime::default())
            .expect("second compile")
            .1;
        assert_eq!(first, second);
    }

    #[test]
    fn fixture_openapi_is_json_object() {
        let (openapi, _, _) = fixture("menagerie");
        assert!(matches!(openapi.0, Value::Object(_)));
    }
}
