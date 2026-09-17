from pathlib import Path


def replace_once(path: Path, old: str, new: str) -> None:
    source = path.read_text()
    if old not in source:
        raise SystemExit(f"migration marker not found in {path}: {old[:80]!r}")
    path.write_text(source.replace(old, new, 1))


# Public owned input contract for the complete-definition generation path.
contracts = Path("src/contracts.rs")
replace_once(
    contracts,
    """}\n\n/// Deterministic inventory of public names selected by lowering.\n""",
    """}\n\n/// Complete owned input for deterministic SDK generation.\n///\n/// Semantic derivation of `SdkDefinition` remains the responsibility of issue #1.\n#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]\n#[serde(deny_unknown_fields)]\npub struct GenerateInput {\n    pub openapi: OpenApi,\n    pub bindings: Bindings,\n    pub definition: SdkDefinition,\n    #[serde(default)]\n    pub runtime: Runtime,\n}\n\n/// Deterministic inventory of public names selected by lowering.\n""",
)

# Make the closed compiler return the stable public output contract.
compiler = Path("src/compiler.rs")
replace_once(
    compiler,
    """use crate::contracts::{Bindings, OpenApi, Runtime, SdkDefinition};\n""",
    """use crate::contracts::{\n    ApiInventory, Bindings, GenerateInput, GeneratedSdk, OpenApi, ResourceInventory, Runtime,\n    SdkDefinition,\n};\n""",
)
replace_once(
    compiler,
    """    Ok((ir, files))\n}\n\n#[cfg(test)]\n""",
    """    Ok((ir, files))\n}\n\npub(crate) fn generate(input: GenerateInput) -> Result<GeneratedSdk> {\n    let GenerateInput {\n        openapi,\n        bindings,\n        definition,\n        runtime,\n    } = input;\n    let (ir, files) = compile(&openapi, &bindings, &definition, &runtime)?;\n    let inventory = ApiInventory {\n        client: ir.client_name.clone(),\n        models: ir.models.iter().map(|model| model.name.clone()).collect(),\n        resources: ir\n            .resources\n            .iter()\n            .map(|resource| ResourceInventory {\n                path: resource.path.clone(),\n                module: resource.module.clone(),\n                name: resource.name.clone(),\n                operations: resource\n                    .operations\n                    .iter()\n                    .map(|operation| operation.name.clone())\n                    .collect(),\n            })\n            .collect(),\n    };\n    Ok(GeneratedSdk { files, inventory })\n}\n\n#[cfg(test)]\n""",
)

# Canonical library surface: a normal Rust API over the same compiler used by the CLI.
Path("src/lib.rs").write_text(
    """//! Backend-neutral Rust SDK derivation and generation.\n//!\n//! The root crate owns complete-definition validation, lowering and deterministic emission.\n//! Concrete OpenAPI-to-Rust backends remain adapters that only produce [`Bindings`].\n\nmod compiler;\nmod contracts;\nmod emit;\nmod error;\n#[allow(dead_code)]\nmod ir;\n#[allow(\n    clippy::collapsible_if,\n    clippy::needless_lifetimes,\n    clippy::unnecessary_unwrap,\n    clippy::useless_format\n)]\nmod lower;\n#[allow(dead_code)]\nmod openapi;\nmod rust_type;\n#[allow(dead_code)]\nmod symbols;\n#[allow(clippy::collapsible_if)]\nmod validation;\n\npub use contracts::{\n    AccessorDefinition, AccessorKindDefinition, ApiInventory, BindingLayout, Bindings,\n    ClientBinding, ClientDefinition, FieldBinding, GenerateInput, GeneratedSdk, MapDefinition,\n    ModelDefinition, OpenApi, OperationBinding, OperationDefinition, ParameterBinding,\n    ResourceDefinition, ResourceInventory, Runtime, ScalarEnumDefinition, SdkDefinition,\n    SimpleUnionDefinition, SimpleUnionVariant, StreamBinding, StreamDefinition, UnionDefinition,\n    UnionFactoryDefinition, VariantBinding,\n};\npub use error::{Diagnostic, GenerationError};\npub use rust_type::{Type, TypeKind, parse_type};\n\n/// Validate and generate an SDK from an explicit complete definition.\n///\n/// This is the behavior-equivalent migration endpoint. `derive()` is intentionally\n/// reserved for the semantic expansion tracked by issue #1.\npub fn generate(input: GenerateInput) -> Result<GeneratedSdk, GenerationError> {\n    compiler::generate(input)\n}\n"""
)

# Preserve the exact Python-era OpenAPI operation/discriminator behavior before switching runtimes.
openapi = Path("src/openapi.rs")
replace_once(
    openapi,
    """                for method in [\n                    \"get\", \"put\", \"post\", \"delete\", \"options\", \"head\", \"patch\", \"trace\",\n                ] {\n""",
    """                for method in [\"get\", \"put\", \"post\", \"delete\", \"patch\", \"head\", \"options\"] {\n""",
)
replace_once(
    openapi,
    """        let discriminator = schema\n            .pointer(\"/discriminator/propertyName\")\n            .and_then(Value::as_str)\n            .ok_or_else(|| {\n                error(\n                    \"openapi.union_discriminator\",\n                    format!(\"OpenAPI union discriminator missing at {root}\"),\n                )\n            })?;\n\n""",
    """        let explicit_discriminator = schema\n            .pointer(\"/discriminator/propertyName\")\n            .and_then(Value::as_str);\n        if explicit_discriminator.is_none() {\n            let referenced: Vec<String> = branches\n                .iter()\n                .filter_map(ref_name)\n                .map(str::to_owned)\n                .collect();\n            if !referenced.is_empty() && referenced.len() == branches.len() {\n                let mut candidates: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();\n                for payload in &referenced {\n                    if let Some(properties) = self\n                        .schema(payload)?\n                        .get(\"properties\")\n                        .and_then(Value::as_object)\n                    {\n                        for (property_name, property_schema) in properties {\n                            if let Some(value) = property_schema.get(\"const\") {\n                                let tag = match value {\n                                    Value::String(value) => value.clone(),\n                                    Value::Bool(true) => \"True\".into(),\n                                    Value::Bool(false) => \"False\".into(),\n                                    Value::Null => \"None\".into(),\n                                    value => value.to_string(),\n                                };\n                                candidates\n                                    .entry(property_name.clone())\n                                    .or_default()\n                                    .insert(tag, payload.clone());\n                            }\n                        }\n                    }\n                }\n                let mut complete = candidates\n                    .into_iter()\n                    .filter(|(_, mapping)| mapping.len() == referenced.len());\n                if let Some((property_name, mapping)) = complete.next()\n                    && complete.next().is_none()\n                {\n                    return Ok((property_name, mapping));\n                }\n            }\n        }\n        let discriminator = explicit_discriminator.ok_or_else(|| {\n            error(\n                \"openapi.union_discriminator\",\n                format!(\"OpenAPI union discriminator missing at {root}\"),\n            )\n        })?;\n\n""",
)
source = openapi.read_text()
if "fn infers_unique_const_discriminator_without_discriminator_object" not in source:
    tests = r'''

    #[test]
    fn infers_unique_const_discriminator_without_discriminator_object() {
        let value = serde_json::json!({
            "components": {"schemas": {
                "Animal": {"oneOf": [
                    {"$ref": "#/components/schemas/Cat"},
                    {"$ref": "#/components/schemas/Dog"}
                ]},
                "Cat": {"type": "object", "properties": {"kind": {"const": "cat"}}},
                "Dog": {"type": "object", "properties": {"kind": {"const": "dog"}}}
            }}
        });
        let index = OpenApiIndex::new(&OpenApi(value)).expect("valid index");
        let (property, mapping) = index.union("Animal", &[]).expect("inferred union");
        assert_eq!(property, "kind");
        assert_eq!(mapping["cat"], "Cat");
        assert_eq!(mapping["dog"], "Dog");
    }

    #[test]
    fn ignores_trace_operations_to_match_existing_contract() {
        let value = serde_json::json!({
            "paths": {"/trace": {"trace": {"operationId": "trace_only"}}}
        });
        let index = OpenApiIndex::new(&OpenApi(value)).expect("valid index");
        assert!(!index.operations.contains_key("trace_only"));
    }
'''
    head, tail = source.rsplit("\n}", 1)
    openapi.write_text(head + tests + "\n}" + tail)

# Deterministic repository-facing CLI over the library API.
Path("src/main.rs").write_text(
    r'''use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process;

use serde::de::DeserializeOwned;
use serde_json::json;
use rust_sdk_generator::{
    generate, ApiInventory, Bindings, GenerateInput, GenerationError, OpenApi, Runtime,
    SdkDefinition,
};

const USAGE: &str = "usage:\n  rust-sdk-generator generate --openapi FILE --bindings FILE --definition FILE --output DIR [--runtime FILE] [--inventory FILE]\n  rust-sdk-generator check --openapi FILE --bindings FILE --definition FILE [--runtime FILE] [--inventory FILE]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandKind {
    Generate,
    Check,
}

#[derive(Debug)]
struct Cli {
    command: CommandKind,
    openapi: PathBuf,
    bindings: PathBuf,
    definition: PathBuf,
    runtime: Option<PathBuf>,
    output: Option<PathBuf>,
    inventory: Option<PathBuf>,
}

#[derive(Debug)]
struct CliError {
    code: String,
    message: String,
    path: Option<String>,
}

impl CliError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            path: None,
        }
    }

    fn at(code: &str, path: &Path, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            path: Some(path.display().to_string()),
        }
    }
}

impl From<GenerationError> for CliError {
    fn from(error: GenerationError) -> Self {
        Self {
            code: error.diagnostic.code,
            message: error.diagnostic.message,
            path: error.diagnostic.path,
        }
    }
}

fn parse_args<I>(args: I) -> Result<Option<Cli>, CliError>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Err(CliError::new("cli.usage", USAGE));
    };
    if command == "--help" || command == "-h" || command == "help" {
        return Ok(None);
    }
    let command = match command.as_str() {
        "generate" => CommandKind::Generate,
        "check" => CommandKind::Check,
        _ => return Err(CliError::new("cli.usage", USAGE)),
    };

    let mut options = BTreeMap::new();
    while let Some(flag) = args.next() {
        if !flag.starts_with("--") {
            return Err(CliError::new(
                "cli.usage",
                format!("unexpected argument {flag:?}\n{USAGE}"),
            ));
        }
        let value = args.next().ok_or_else(|| {
            CliError::new("cli.usage", format!("missing value for {flag}\n{USAGE}"))
        })?;
        if options.insert(flag.clone(), value).is_some() {
            return Err(CliError::new(
                "cli.usage",
                format!("duplicate option {flag}\n{USAGE}"),
            ));
        }
    }

    let required = |name: &str| {
        options
            .get(name)
            .map(PathBuf::from)
            .ok_or_else(|| CliError::new("cli.usage", format!("missing {name}\n{USAGE}")))
    };
    for flag in options.keys() {
        if !matches!(
            flag.as_str(),
            "--openapi" | "--bindings" | "--definition" | "--runtime" | "--output" | "--inventory"
        ) {
            return Err(CliError::new(
                "cli.usage",
                format!("unknown option {flag}\n{USAGE}"),
            ));
        }
    }

    let output = options.get("--output").map(PathBuf::from);
    if command == CommandKind::Generate && output.is_none() {
        return Err(CliError::new(
            "cli.usage",
            format!("generate requires --output\n{USAGE}"),
        ));
    }
    if command == CommandKind::Check && output.is_some() {
        return Err(CliError::new(
            "cli.usage",
            format!("check does not accept --output\n{USAGE}"),
        ));
    }

    Ok(Some(Cli {
        command,
        openapi: required("--openapi")?,
        bindings: required("--bindings")?,
        definition: required("--definition")?,
        runtime: options.get("--runtime").map(PathBuf::from),
        output,
        inventory: options.get("--inventory").map(PathBuf::from),
    }))
}

fn read_json<T: DeserializeOwned>(path: &Path, kind: &str) -> Result<T, CliError> {
    let source = fs::read_to_string(path).map_err(|error| {
        CliError::at(
            "cli.io",
            path,
            format!("failed to read {kind}: {error}"),
        )
    })?;
    serde_json::from_str(&source).map_err(|error| {
        CliError::at(
            "cli.json",
            path,
            format!("failed to parse {kind} JSON: {error}"),
        )
    })
}

fn inventory_json(inventory: &ApiInventory) -> Result<Vec<u8>, CliError> {
    let mut bytes = serde_json::to_vec_pretty(inventory)
        .map_err(|error| CliError::new("cli.json", error.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn safe_relative_path(name: &str) -> Result<&Path, CliError> {
    let path = Path::new(name);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(CliError::new(
            "cli.output_path",
            format!("generated path escapes output directory: {name}"),
        ));
    }
    Ok(path)
}

fn write_generated(output: &Path, files: &BTreeMap<String, String>) -> Result<(), CliError> {
    for (name, source) in files {
        let path = output.join(safe_relative_path(name)?);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                CliError::at("cli.io", parent, format!("failed to create output directory: {error}"))
            })?;
        }
        fs::write(&path, source).map_err(|error| {
            CliError::at("cli.io", &path, format!("failed to write generated source: {error}"))
        })?;
    }
    Ok(())
}

fn run(cli: Cli) -> Result<(), CliError> {
    let openapi: OpenApi = read_json(&cli.openapi, "OpenAPI")?;
    let bindings: Bindings = read_json(&cli.bindings, "Bindings")?;
    let definition: SdkDefinition = read_json(&cli.definition, "SdkDefinition")?;
    let runtime = match &cli.runtime {
        Some(path) => read_json(path, "Runtime")?,
        None => Runtime::default(),
    };
    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition,
        runtime,
    })?;

    if let Some(output) = &cli.output {
        write_generated(output, &generated.files)?;
    }
    let inventory = inventory_json(&generated.inventory)?;
    if let Some(path) = &cli.inventory {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|error| {
                CliError::at("cli.io", parent, format!("failed to create inventory directory: {error}"))
            })?;
        }
        fs::write(path, &inventory).map_err(|error| {
            CliError::at("cli.io", path, format!("failed to write API inventory: {error}"))
        })?;
    }
    io::stdout()
        .write_all(&inventory)
        .map_err(|error| CliError::new("cli.io", format!("failed to write stdout: {error}")))?;
    Ok(())
}

fn emit_error(error: &CliError) {
    let value = json!({
        "code": error.code,
        "message": error.message,
        "path": error.path,
    });
    let _ = writeln!(io::stderr(), "{value}");
}

fn main() {
    match parse_args(env::args().skip(1)) {
        Ok(None) => println!("{USAGE}"),
        Ok(Some(cli)) => {
            if let Err(error) = run(cli) {
                emit_error(&error);
                process::exit(2);
            }
        }
        Err(error) => {
            emit_error(&error);
            process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_requires_output() {
        let error = parse_args([
            "generate".into(),
            "--openapi".into(),
            "openapi.json".into(),
            "--bindings".into(),
            "bindings.json".into(),
            "--definition".into(),
            "definition.json".into(),
        ])
        .expect_err("missing output must fail");
        assert_eq!(error.code, "cli.usage");
    }

    #[test]
    fn check_rejects_output() {
        let error = parse_args([
            "check".into(),
            "--openapi".into(),
            "openapi.json".into(),
            "--bindings".into(),
            "bindings.json".into(),
            "--definition".into(),
            "definition.json".into(),
            "--output".into(),
            "out".into(),
        ])
        .expect_err("check output must fail");
        assert_eq!(error.code, "cli.usage");
    }
}
'''
)

# Mechanical proof that CLI and library execute the exact same generation path.
Path("tests/rust_surface.rs").write_text(
    r'''use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use rust_sdk_generator::{
    generate, ApiInventory, Bindings, GenerateInput, OpenApi, Runtime, SdkDefinition,
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
'''
)
