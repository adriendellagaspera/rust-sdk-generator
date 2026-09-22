use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;

use rust_sdk_generator::{
    ApiInventory, Bindings, DerivationError, DeriveInput, GenerateInput, GenerationError, OpenApi,
    PublicSdkSurface, Runtime, SdkDefinition, SdkOverrides, derive, generate,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

mod output;

const USAGE: &str = "usage:\n  rust-sdk-generator derive --openapi FILE --bindings FILE [--surface FILE] [--overrides FILE] [--definition-output FILE]\n  rust-sdk-generator generate --openapi FILE --bindings FILE --definition FILE --output DIR [--runtime FILE] [--inventory FILE]\n  rust-sdk-generator check --openapi FILE --bindings FILE --definition FILE [--runtime FILE] [--inventory FILE]\n  rust-sdk-generator check-generated --openapi FILE --bindings FILE --definition FILE --output DIR [--runtime FILE]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandKind {
    Derive,
    Generate,
    Check,
    CheckGenerated,
}

#[derive(Debug)]
struct Cli {
    command: CommandKind,
    openapi: PathBuf,
    bindings: PathBuf,
    surface: Option<PathBuf>,
    overrides: Option<PathBuf>,
    definition: Option<PathBuf>,
    definition_output: Option<PathBuf>,
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

impl From<DerivationError> for CliError {
    fn from(error: DerivationError) -> Self {
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
        "derive" => CommandKind::Derive,
        "generate" => CommandKind::Generate,
        "check" => CommandKind::Check,
        "check-generated" => CommandKind::CheckGenerated,
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

    let allowed: &[&str] = match command {
        CommandKind::Derive => &[
            "--openapi",
            "--bindings",
            "--surface",
            "--overrides",
            "--definition-output",
        ],
        CommandKind::Generate | CommandKind::Check | CommandKind::CheckGenerated => &[
            "--openapi",
            "--bindings",
            "--definition",
            "--runtime",
            "--output",
            "--inventory",
        ],
    };
    for flag in options.keys() {
        if !allowed.contains(&flag.as_str()) {
            return Err(CliError::new(
                "cli.usage",
                format!("unknown option {flag} for command\n{USAGE}"),
            ));
        }
    }

    let output = options.get("--output").map(PathBuf::from);
    if matches!(command, CommandKind::Generate | CommandKind::CheckGenerated) && output.is_none() {
        return Err(CliError::new(
            "cli.usage",
            format!("{command:?} requires --output\n{USAGE}"),
        ));
    }
    if command == CommandKind::Check && output.is_some() {
        return Err(CliError::new(
            "cli.usage",
            format!("check does not accept --output\n{USAGE}"),
        ));
    }

    if command == CommandKind::CheckGenerated && options.contains_key("--inventory") {
        return Err(CliError::new(
            "cli.usage",
            format!("check-generated is read-only and does not accept --inventory\n{USAGE}"),
        ));
    }

    let definition = match command {
        CommandKind::Derive => None,
        CommandKind::Generate | CommandKind::Check | CommandKind::CheckGenerated => {
            Some(required("--definition")?)
        }
    };

    Ok(Some(Cli {
        command,
        openapi: required("--openapi")?,
        bindings: required("--bindings")?,
        surface: options.get("--surface").map(PathBuf::from),
        overrides: options.get("--overrides").map(PathBuf::from),
        definition,
        definition_output: options.get("--definition-output").map(PathBuf::from),
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

fn json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, CliError> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| CliError::new("cli.json", error.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn inventory_json(inventory: &ApiInventory) -> Result<Vec<u8>, CliError> {
    json_bytes(inventory)
}

fn write_json_file(path: &Path, bytes: &[u8], kind: &str) -> Result<(), CliError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|error| {
            CliError::at(
                "cli.io",
                parent,
                format!("failed to create {kind} directory: {error}"),
            )
        })?;
    }
    fs::write(path, bytes)
        .map_err(|error| CliError::at("cli.io", path, format!("failed to write {kind}: {error}")))
}

fn run(cli: Cli) -> Result<i32, CliError> {
    let openapi: OpenApi = read_json(&cli.openapi, "OpenAPI")?;
    let bindings: Bindings = read_json(&cli.bindings, "Bindings")?;

    if cli.command == CommandKind::Derive {
        let surface = match &cli.surface {
            Some(path) => read_json(path, "PublicSdkSurface")?,
            None => PublicSdkSurface::default(),
        };
        let overrides = match &cli.overrides {
            Some(path) => read_json(path, "SdkOverrides")?,
            None => SdkOverrides::default(),
        };
        let derivation = derive(DeriveInput {
            openapi,
            bindings,
            surface,
            overrides,
        })?;
        if let Some(path) = &cli.definition_output {
            write_json_file(path, &json_bytes(&derivation.definition)?, "SdkDefinition")?;
        }
        io::stdout()
            .write_all(&json_bytes(&derivation)?)
            .map_err(|error| CliError::new("cli.io", format!("failed to write stdout: {error}")))?;
        return Ok(0);
    }

    let definition_path = cli.definition.as_deref().expect("generation definition");
    let definition: SdkDefinition = read_json(definition_path, "SdkDefinition")?;
    let runtime = match &cli.runtime {
        Some(path) => read_json(path, "Runtime")?,
        None => Runtime::default(),
    };
    let marker = runtime.generated_marker.clone();
    let generated = generate(GenerateInput {
        openapi,
        bindings,
        definition,
        runtime,
    })?;

    if cli.command == CommandKind::CheckGenerated {
        let output = cli.output.as_deref().expect("checked output");
        let diff = output::compare(output, &generated.files, &marker)?;
        io::stdout()
            .write_all(&json_bytes(&diff)?)
            .map_err(|error| CliError::new("cli.io", format!("failed to write stdout: {error}")))?;
        return Ok(if diff.is_clean() { 0 } else { 1 });
    }
    if let Some(output) = &cli.output {
        output::publish(output, &generated.files, &marker)?;
    }
    let inventory = inventory_json(&generated.inventory)?;
    if let Some(path) = &cli.inventory {
        write_json_file(path, &inventory, "API inventory")?;
    }
    io::stdout()
        .write_all(&inventory)
        .map_err(|error| CliError::new("cli.io", format!("failed to write stdout: {error}")))?;
    Ok(0)
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
        Ok(Some(cli)) => match run(cli) {
            Ok(0) => {}
            Ok(code) => process::exit(code),
            Err(error) => {
                emit_error(&error);
                process::exit(2);
            }
        },
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

    #[test]
    fn derive_does_not_accept_generation_definition() {
        let error = parse_args([
            "derive".into(),
            "--openapi".into(),
            "openapi.json".into(),
            "--bindings".into(),
            "bindings.json".into(),
            "--definition".into(),
            "definition.json".into(),
        ])
        .expect_err("derive definition must fail");
        assert_eq!(error.code, "cli.usage");
    }
}
