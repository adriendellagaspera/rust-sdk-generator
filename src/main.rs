use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process;

use rust_sdk_generator::{
    ApiInventory, Bindings, GenerateInput, GenerationError, OpenApi, Runtime, SdkDefinition,
    generate,
};
use serde::de::DeserializeOwned;
use serde_json::json;

const USAGE: &str = "usage:\n  rust-sdk-generator generate --openapi FILE --bindings FILE --definition FILE --output DIR [--runtime FILE] [--inventory FILE]\n  rust-sdk-generator check --openapi FILE --bindings FILE --definition FILE [--runtime FILE] [--inventory FILE]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandKind {
    Generate,
    Check,
}

#[derive(Debug)]
struct Cli {
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
        openapi: required("--openapi")?,
        bindings: required("--bindings")?,
        definition: required("--definition")?,
        runtime: options.get("--runtime").map(PathBuf::from),
        output,
        inventory: options.get("--inventory").map(PathBuf::from),
    }))
}

fn read_json<T: DeserializeOwned>(path: &Path, kind: &str) -> Result<T, CliError> {
    let source = fs::read_to_string(path)
        .map_err(|error| CliError::at("cli.io", path, format!("failed to read {kind}: {error}")))?;
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
                CliError::at(
                    "cli.io",
                    parent,
                    format!("failed to create output directory: {error}"),
                )
            })?;
        }
        fs::write(&path, source).map_err(|error| {
            CliError::at(
                "cli.io",
                &path,
                format!("failed to write generated source: {error}"),
            )
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
                CliError::at(
                    "cli.io",
                    parent,
                    format!("failed to create inventory directory: {error}"),
                )
            })?;
        }
        fs::write(path, &inventory).map_err(|error| {
            CliError::at(
                "cli.io",
                path,
                format!("failed to write API inventory: {error}"),
            )
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
