//! Local-first, own-OpenAPI adopter CLI. The library remains backend-neutral:
//! driver -> canonical Bindings v3 -> root derive -> root generate/check-generated.
#[path = "sdk_adopter/driver.rs"]
mod driver;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use driver::{BackendDriver, BackendLock, UpstreamDriver};
use rust_sdk_generator::{
    Bindings, Derivation, DerivationStatus, DeriveInput, OpenApi, PublicSdkSurface, SdkOverrides,
    derive,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, Failure>;

#[derive(Debug)]
struct Failure {
    stage: &'static str,
    message: String,
}
fn err(stage: &'static str, message: impl std::fmt::Display) -> Failure {
    Failure {
        stage,
        message: message.to_string(),
    }
}

fn run(stage: &'static str, command: &mut Command) -> Result<Output> {
    let result = command
        .output()
        .map_err(|e| err(stage, format!("{command:?}: {e}")))?;
    if !result.status.success() {
        return Err(err(
            stage,
            format!(
                "{command:?} exited {}\nstdout:\n{}\nstderr:\n{}",
                result.status,
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            ),
        ));
    }
    Ok(result)
}

fn write_if_changed(path: &Path, bytes: &[u8], stage: &'static str) -> Result<()> {
    if fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| err(stage, e))?;
    }
    fs::write(path, bytes).map_err(|e| err(stage, format!("{}: {e}", path.display())))
}
fn json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| err("report.json", e))?;
    bytes.push(b'\n');
    Ok(bytes)
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn read(path: &Path, stage: &'static str) -> Result<Vec<u8>> {
    fs::read(path).map_err(|e| err(stage, format!("{}: {e}", path.display())))
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8], stage: &'static str) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| err(stage, e))
}
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn required_pin() -> Result<String> {
    let pin: Value = parse(
        &read(
            &root().join("openapi-to-rust-bindings/DEFAULT_BACKEND.json"),
            "backend.pin",
        )?,
        "backend.pin",
    )?;
    let commit = pin["commit"]
        .as_str()
        .ok_or_else(|| err("backend.pin", "missing immutable default revision"))?;
    if pin["repository"] != driver::REPOSITORY
        || commit.len() != 40
        || !commit.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(err(
            "backend.pin",
            "invalid immutable upstream revision/repository",
        ));
    }
    Ok(commit.to_owned())
}
fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        && !value.contains("--")
}
fn safe_relative(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    path: String,
    sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    schema_version: u32,
    source: Evidence,
    backend: BackendLock,
    template_version: u32,
    crate_name: String,
    raw_output: String,
    sdk_output: String,
    dependency_fragment_sha256: String,
    surface: Option<Evidence>,
    overrides: Option<Evidence>,
    coverage: BTreeMap<String, String>,
    owned_raw: BTreeMap<String, String>,
}
fn validate_recipe(recipe: &Recipe) -> Result<()> {
    if recipe.schema_version != 1
        || recipe.template_version != driver::TEMPLATE_VERSION
        || recipe.source.path != "openapi.json"
        || recipe.raw_output != "src/generated"
        || recipe.sdk_output != "src/sdk"
        || !safe_name(&recipe.crate_name)
    {
        return Err(err(
            "recipe.invalid",
            "unsupported recipe/template/layout/crate identity; migrate explicitly",
        ));
    }
    for evidence in [recipe.surface.as_ref(), recipe.overrides.as_ref()]
        .into_iter()
        .flatten()
    {
        if !safe_relative(&evidence.path) || evidence.sha256.len() != 64 {
            return Err(err(
                "recipe.invalid",
                "invalid relative naming-evidence path or digest",
            ));
        }
    }
    if recipe.dependency_fragment_sha256.len() != 64
        || recipe.source.sha256.len() != 64
        || recipe.owned_raw.values().any(|digest| digest.len() != 64)
    {
        return Err(err(
            "recipe.invalid",
            "missing or invalid generation evidence digest",
        ));
    }
    driver::verify_lock(&recipe.backend, &recipe.backend.revision)
}
fn coverage(report: &Derivation) -> Result<BTreeMap<String, String>> {
    if report.report.schema_version != 1 || report.report.operations.is_empty() {
        return Err(err(
            "derive.coverage",
            "missing exhaustive v1 operation report",
        ));
    }
    let mut decisions = BTreeMap::new();
    for (id, entry) in &report.report.operations {
        let status = match entry.status {
            DerivationStatus::Derived => "derived",
            DerivationStatus::Overridden => "overridden",
            DerivationStatus::Excluded => "excluded",
            DerivationStatus::Rejected => {
                return Err(err(
                    "derive.unsupported",
                    format!(
                        "operationId {id}: rejected ({}): {}; inspect the emitted derivation report and supply only structurally supported evidence",
                        entry.reason.code,
                        entry.reason.detail.as_deref().unwrap_or("")
                    ),
                ));
            }
        };
        decisions.insert(id.clone(), format!("{status}:{}", entry.reason.code));
    }
    Ok(decisions)
}
fn read_evidence(
    crate_dir: &Path,
    evidence: &Option<Evidence>,
    stage: &'static str,
) -> Result<Option<Vec<u8>>> {
    evidence.as_ref().map(|e| {
        if !safe_relative(&e.path) { return Err(err(stage, "evidence path escapes crate")); }
        let bytes = read(&crate_dir.join(&e.path), stage)?;
        if hash(&bytes) != e.sha256 {
            return Err(err(stage, format!("{} no longer matches locked evidence; review/migrate the recipe explicitly", e.path)));
        }
        Ok(bytes)
    }).transpose()
}
fn source_validation(source: &[u8]) -> Result<OpenApi> {
    let value: Value = parse(source, "source.json")?;
    if !value.is_object()
        || value["openapi"]
            .as_str()
            .is_none_or(|v| !v.starts_with("3.1."))
        || !value["paths"].is_object()
    {
        return Err(err(
            "source.openapi",
            "require a local OpenAPI 3.1 JSON object with paths; URL/YAML and unsupported versions are not accepted",
        ));
    }
    Ok(OpenApi(value))
}
fn cache_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("RUST_SDK_ADOPTER_CACHE") {
        return Ok(PathBuf::from(path));
    }
    Ok(PathBuf::from(
        env::var_os("HOME")
            .ok_or_else(|| err("cache.path", "set HOME or RUST_SDK_ADOPTER_CACHE"))?,
    )
    .join(".cache/rust-sdk-adopter/v1"))
}
fn tools(offline: bool) -> Result<(PathBuf, PathBuf)> {
    let root = root();
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                root.join(path)
            }
        })
        .unwrap_or_else(|| root.join("target"));
    for (package, stage) in [
        ("rust-sdk-generator", "tools.generator_build"),
        ("openapi-to-rust-bindings", "tools.adapter_build"),
    ] {
        let mut command = Command::new("cargo");
        command
            .args([
                "build",
                "--locked",
                "-p",
                package,
                "--bin",
                package,
                "--manifest-path",
            ])
            .arg(root.join("Cargo.toml"));
        if offline {
            command.arg("--offline");
        }
        run(stage, &mut command)?;
    }
    Ok((
        target
            .join("debug")
            .join(format!("rust-sdk-generator{}", env::consts::EXE_SUFFIX)),
        target.join("debug").join(format!(
            "openapi-to-rust-bindings{}",
            env::consts::EXE_SUFFIX
        )),
    ))
}
fn root_command(generator: &Path, command: &str, work: &Path, output: Option<&Path>) -> Command {
    let mut cmd = Command::new(generator);
    cmd.arg(command)
        .arg("--openapi")
        .arg(work.join("effective-openapi.json"))
        .arg("--bindings")
        .arg(work.join("rust-bindings.json"))
        .arg("--definition")
        .arg(work.join("definition.json"));
    if let Some(out) = output {
        cmd.arg("--output").arg(out);
    }
    cmd
}
fn dependencies(fragment: &str, crate_name: &str) -> String {
    let mut extras = String::new();
    for (name, version) in [("futures-util", "0.3"), ("bytes", "1")] {
        let present = fragment.lines().any(|line| {
            line.trim_start()
                .strip_prefix(name)
                .is_some_and(|tail| tail.trim_start().starts_with('='))
        });
        if !present {
            extras.push_str(&format!("{name} = \"{version}\"\n"));
        }
    }
    let included = fragment.replacen("[dependencies]", &format!("[dependencies]\n{extras}"), 1);
    format!(
        "[package]\nname = \"{crate_name}\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[workspace]\n\n{included}\n\n[dev-dependencies]\ntokio = {{ version = \"1\", features = [\"macros\", \"rt-multi-thread\"] }}\n"
    )
}
fn create_starter(dir: &Path, recipe: &Recipe, fragment: &str) -> Result<()> {
    write_if_changed(
        &dir.join("Cargo.toml"),
        dependencies(fragment, &recipe.crate_name).as_bytes(),
        "template.manifest",
    )?;
    write_if_changed(&dir.join("src/lib.rs"), b"//! Consumer-owned crate root; review authentication and public API before release.\npub mod generated;\npub mod sdk;\n", "template.lib")?;
    write_if_changed(
        &dir.join("src/sdk/error.rs"),
        include_bytes!("../../examples/independent-sdk/consumer/src/sdk/error.rs"),
        "template.runtime",
    )?;
    Ok(())
}
fn copy_tree(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).map_err(|e| err("stage.copy", e))?;
    for entry in fs::read_dir(source).map_err(|e| err("stage.copy", e))? {
        let entry = entry.map_err(|e| err("stage.copy", e))?;
        let path = entry.path();
        let name = entry.file_name();
        if name == "target" || name == ".sdkgen" {
            continue;
        }
        let target = dest.join(name);
        let kind = entry.file_type().map_err(|e| err("stage.copy", e))?;
        if kind.is_symlink() || (!kind.is_file() && !kind.is_dir()) {
            return Err(err(
                "stage.copy",
                format!("symlink/special file in crate: {}", path.display()),
            ));
        }
        if kind.is_dir() {
            copy_tree(&path, &target)?;
        } else {
            fs::copy(&path, &target).map_err(|e| err("stage.copy", e))?;
        }
    }
    Ok(())
}
struct Stage(PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn stage_path(dest: &Path) -> Result<Stage> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| err("stage.create", e))?
        .as_nanos();
    let stage = dest.with_file_name(format!(".sdk-adopter-stage-{}-{stamp}", std::process::id()));
    fs::create_dir(&stage).map_err(|e| err("stage.create", e))?;
    Ok(Stage(stage))
}
fn raw_ownership(
    crate_dir: &Path,
    recipe: &Recipe,
    new: &BTreeMap<String, Vec<u8>>,
) -> Result<BTreeMap<String, String>> {
    let generated = crate_dir.join(&recipe.raw_output);
    let mut current = BTreeMap::new();
    if generated.is_dir() {
        for entry in fs::read_dir(&generated).map_err(|e| err("raw.ownership", e))? {
            let entry = entry.map_err(|e| err("raw.ownership", e))?;
            let kind = entry.file_type().map_err(|e| err("raw.ownership", e))?;
            if !kind.is_file() {
                return Err(err(
                    "raw.ownership",
                    "raw output contains a symlink, directory or special file",
                ));
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| err("raw.ownership", "non-UTF-8 raw filename"))?;
            let digest = hash(&read(&entry.path(), "raw.ownership")?);
            match recipe.owned_raw.get(&name) {
                Some(owned) if owned == &digest => {}
                _ => {
                    return Err(err(
                        "raw.ownership",
                        format!(
                            "unmanaged or modified raw file {}: refusing to overwrite/delete consumer code",
                            entry.path().display()
                        ),
                    ));
                }
            }
            current.insert(name, digest);
        }
    }
    for name in recipe.owned_raw.keys() {
        if !current.contains_key(name) {
            return Err(err(
                "raw.ownership",
                format!("missing previously owned raw file {name}"),
            ));
        }
    }
    Ok(new
        .iter()
        .map(|(name, bytes)| (name.clone(), hash(bytes)))
        .collect())
}
fn publish_raw(
    dest: &Path,
    old: &BTreeMap<String, String>,
    new: &BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    for name in old.keys() {
        if !new.contains_key(name) {
            fs::remove_file(dest.join(name)).map_err(|e| err("raw.publish", e))?;
        }
    }
    for (name, bytes) in new {
        write_if_changed(&dest.join(name), bytes, "raw.publish")?;
    }
    Ok(())
}
fn compile(dir: &Path, offline: bool) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.args(["test", "--locked", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .arg("--all-targets");
    if offline {
        cmd.arg("--offline");
    }
    // The staging crate is isolated from the root workspace and has its own lock.
    if !dir.join("Cargo.lock").is_file() {
        let mut lock = Command::new("cargo");
        lock.arg("generate-lockfile")
            .args(["--manifest-path"])
            .arg(dir.join("Cargo.toml"));
        if offline {
            lock.arg("--offline");
        }
        run("crate.lockfile", &mut lock)?;
    }
    run("crate.compile", &mut cmd)?;
    Ok(())
}
#[derive(Debug)]
struct Cli {
    init: bool,
    check: bool,
    offline: bool,
    accept_coverage: bool,
    output: PathBuf,
    source: Option<PathBuf>,
    name: Option<String>,
    surface: Option<PathBuf>,
    overrides: Option<PathBuf>,
}
fn args() -> Result<Cli> {
    let mut iter = env::args().skip(1);
    let command = iter.next().ok_or_else(|| err("cli.usage", "sdk-adopter init --openapi FILE --output DIR --name CRATE [--surface FILE] [--overrides FILE] [--offline] | sync --crate DIR [--check] [--accept-coverage] [--offline]"))?;
    let init = match command.as_str() {
        "init" => true,
        "sync" => false,
        _ => return Err(err("cli.usage", "expected init or sync")),
    };
    let mut values = BTreeMap::new();
    let mut check = false;
    let mut offline = false;
    let mut accept_coverage = false;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--offline" => offline = true,
            "--accept-coverage" => accept_coverage = true,
            _ if arg.starts_with("--") => {
                let value = iter
                    .next()
                    .ok_or_else(|| err("cli.usage", format!("missing {arg} value")))?;
                if values.insert(arg, value).is_some() {
                    return Err(err("cli.usage", "duplicate CLI argument"));
                }
            }
            _ => return Err(err("cli.usage", format!("unexpected argument {arg}"))),
        }
    }
    if (init
        && (check
            || accept_coverage
            || values.keys().any(|k| {
                ![
                    "--openapi",
                    "--output",
                    "--name",
                    "--surface",
                    "--overrides",
                ]
                .contains(&k.as_str())
            })))
        || (!init && values.keys().any(|k| k != "--crate"))
        || (check && accept_coverage)
    {
        return Err(err("cli.usage", "invalid command options"));
    }
    let required = if init { "--output" } else { "--crate" };
    let output = values
        .remove(required)
        .map(PathBuf::from)
        .ok_or_else(|| err("cli.usage", format!("missing {required}")))?;
    let source = values.remove("--openapi").map(PathBuf::from);
    let name = values.remove("--name");
    if init && (source.is_none() || name.as_ref().is_none_or(|n| !safe_name(n))) {
        return Err(err(
            "cli.usage",
            "init requires --openapi local JSON and --name valid Rust crate identity",
        ));
    }
    Ok(Cli {
        init,
        check,
        offline,
        accept_coverage,
        output,
        source,
        name,
        surface: values.remove("--surface").map(PathBuf::from),
        overrides: values.remove("--overrides").map(PathBuf::from),
    })
}
fn main_inner() -> Result<()> {
    let cli = args()?;
    let output = if cli.init {
        cli.output.clone()
    } else {
        cli.output
            .canonicalize()
            .map_err(|e| err("recipe.path", e))?
    };
    if cli.init && output.exists() {
        return Err(err(
            "init.destination",
            format!(
                "destination {} already exists; refusing to overwrite",
                output.display()
            ),
        ));
    }
    let old = if cli.init {
        None
    } else {
        let recipe: Recipe = parse(
            &read(&output.join("sdkgen.lock.json"), "recipe.read")?,
            "recipe.json",
        )?;
        validate_recipe(&recipe)?;
        Some(recipe)
    };
    let mut recipe = if let Some(old) = old.clone() {
        old
    } else {
        Recipe {
            schema_version: 1,
            source: Evidence {
                path: "openapi.json".into(),
                sha256: String::new(),
            },
            backend: driver::default_lock(&required_pin()?),
            template_version: driver::TEMPLATE_VERSION,
            crate_name: cli.name.clone().expect("init crate name"),
            raw_output: "src/generated".into(),
            sdk_output: "src/sdk".into(),
            dependency_fragment_sha256: String::new(),
            surface: None,
            overrides: None,
            coverage: BTreeMap::new(),
            owned_raw: BTreeMap::new(),
        }
    };
    driver::verify_lock(&recipe.backend, &recipe.backend.revision)?;
    let source = if let Some(path) = &cli.source {
        read(path, "source.read")?
    } else {
        read(&output.join(&recipe.source.path), "source.read")?
    };
    let openapi = source_validation(&source)?;
    let stage = stage_path(&output)?;
    if old.is_some() {
        copy_tree(&output, &stage.0)?;
    } else {
        fs::create_dir_all(stage.0.join("src")).map_err(|e| err("stage.create", e))?;
    }
    write_if_changed(&stage.0.join(&recipe.source.path), &source, "source.stage")?;
    if let Some(path) = &cli.surface {
        let bytes = read(path, "evidence.read")?;
        write_if_changed(&stage.0.join("surface.json"), &bytes, "evidence.stage")?;
        recipe.surface = Some(Evidence {
            path: "surface.json".into(),
            sha256: hash(&bytes),
        });
    }
    if let Some(path) = &cli.overrides {
        let bytes = read(path, "evidence.read")?;
        write_if_changed(&stage.0.join("overrides.json"), &bytes, "evidence.stage")?;
        recipe.overrides = Some(Evidence {
            path: "overrides.json".into(),
            sha256: hash(&bytes),
        });
    }
    // Existing naming evidence is immutable until an explicit recipe migration.
    let surface: PublicSdkSurface =
        if let Some(bytes) = read_evidence(&stage.0, &recipe.surface, "evidence.surface")? {
            parse(&bytes, "evidence.surface")?
        } else {
            PublicSdkSurface::default()
        };
    let overrides: SdkOverrides =
        if let Some(bytes) = read_evidence(&stage.0, &recipe.overrides, "evidence.overrides")? {
            parse(&bytes, "evidence.overrides")?
        } else {
            SdkOverrides::default()
        };
    let (generator, adapter) = tools(cli.offline)?;
    let cache = cache_dir()?;
    let work = stage.0.join(".sdkgen/work");
    fs::create_dir_all(&work).map_err(|e| err("stage.create", e))?;
    write_if_changed(
        &work.join("effective-openapi.json"),
        &source,
        "source.effective",
    )?;
    let generated =
        UpstreamDriver.generate(&recipe.backend, &work, &cache, cli.offline, &adapter)?;
    let bindings: Bindings = parse(&generated.bindings, "bindings.v3")?;
    if bindings.schema_version != 3 {
        return Err(err(
            "bindings.v3",
            "adapter did not produce canonical Bindings v3",
        ));
    }
    write_if_changed(
        &work.join("rust-bindings.json"),
        &json_bytes(&bindings)?,
        "bindings.write",
    )?;
    // This is the ONLY consumer of canonical Bindings in orchestration. All
    // reconciliation, naming, and structural validation belong to the root.
    let derivation = derive(DeriveInput {
        openapi,
        bindings,
        surface,
        overrides,
    })
    .map_err(|e| {
        err(
            "derive.contract",
            format!("{}: {}", e.diagnostic.code, e.diagnostic.message),
        )
    })?;
    write_if_changed(
        &work.join("derivation.json"),
        &json_bytes(&derivation)?,
        "report.write",
    )?;
    write_if_changed(
        &work.join("definition.json"),
        &json_bytes(&derivation.definition)?,
        "report.write",
    )?;
    let decisions = coverage(&derivation)?;
    if old
        .as_ref()
        .is_some_and(|previous| previous.coverage != decisions)
        && !cli.check
        && !cli.accept_coverage
    {
        println!("{}", serde_json::to_string_pretty(&json!({"coverage_before":old.as_ref().map(|v| &v.coverage),"coverage_after":decisions,"derivation":derivation.report})).map_err(|e| err("report.json", e))?);
        return Err(err(
            "sync.coverage_drift",
            "coverage changed; inspect 'sync --check', then rerun with --accept-coverage only after reviewing the new operations and public surface",
        ));
    }
    recipe.source.sha256 = hash(&source);
    recipe.coverage = decisions;
    let deps_digest = hash(generated.dependencies.as_bytes());
    if let Some(previous) = &old
        && previous.dependency_fragment_sha256 != deps_digest
    {
        return Err(err(
            "sync.dependencies",
            "backend dependency fragment changed; consumer owns Cargo.toml and must review a recipe/runtime migration",
        ));
    }
    recipe.dependency_fragment_sha256 = deps_digest;
    let raw_digests = if let Some(previous) = &old {
        raw_ownership(&output, previous, &generated.rust)?
    } else {
        generated
            .rust
            .iter()
            .map(|(n, b)| (n.clone(), hash(b)))
            .collect()
    };
    recipe.owned_raw = raw_digests;
    let output_dir = output.join(&recipe.sdk_output);
    let diff = if old.is_some() {
        let result = Command::new(&generator)
            .arg("check-generated")
            .arg("--openapi")
            .arg(work.join("effective-openapi.json"))
            .arg("--bindings")
            .arg(work.join("rust-bindings.json"))
            .arg("--definition")
            .arg(work.join("definition.json"))
            .arg("--output")
            .arg(&output_dir)
            .output()
            .map_err(|e| err("sdk.diff", e))?;
        if ![0, 1].contains(&result.status.code().unwrap_or(-1)) {
            return Err(err("sdk.diff", String::from_utf8_lossy(&result.stderr)));
        }
        let value: Value = parse(&result.stdout, "sdk.diff")?;
        if value["conflicts"].as_array().is_some_and(|v| !v.is_empty()) {
            return Err(err(
                "sdk.conflicts",
                format!("handwritten SDK conflict: {value}"),
            ));
        }
        value
    } else {
        json!({"missing":[],"changed":[],"extra":[],"conflicts":[]})
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "derivation":derivation.report,
            "generated_source_diff":diff,
            "raw_source_sha256":recipe.owned_raw,
            "effective_openapi_sha256":recipe.source.sha256
        }))
        .map_err(|e| err("report.json", e))?
    );
    if cli.check {
        return Ok(());
    }
    if old.is_none() {
        create_starter(&stage.0, &recipe, &generated.dependencies)?;
    }
    publish_raw(
        &stage.0.join(&recipe.raw_output),
        &BTreeMap::new(),
        &generated.rust,
    )?;
    let inv_result = run(
        "sdk.generate",
        &mut root_command(
            &generator,
            "generate",
            &work,
            Some(&stage.0.join(&recipe.sdk_output)),
        ),
    )?;
    let inventory: Value = parse(&inv_result.stdout, "sdk.inventory")?;
    write_if_changed(
        &stage.0.join(".sdkgen/inventory.json"),
        &json_bytes(&inventory)?,
        "report.inventory",
    )?;
    // Staging compilation must succeed before any source in the existing crate is changed.
    // This is not a multi-directory atomic transaction; root SDK publication retains
    // the independently tested conflict, lock, and recovery semantics.
    compile(&stage.0, cli.offline)?;
    if let Some(previous) = &old {
        raw_ownership(&output, previous, &generated.rust)?;
        publish_raw(
            &output.join(&recipe.raw_output),
            &previous.owned_raw,
            &generated.rust,
        )?;
        run(
            "sdk.publish",
            &mut root_command(&generator, "generate", &work, Some(&output_dir)),
        )?;
        write_if_changed(
            &output.join(".sdkgen/inventory.json"),
            &json_bytes(&inventory)?,
            "report.inventory",
        )?;
        write_if_changed(
            &output.join(".sdkgen/derivation.json"),
            &json_bytes(&derivation)?,
            "report.write",
        )?;
        write_if_changed(
            &output.join("sdkgen.lock.json"),
            &json_bytes(&recipe)?,
            "recipe.write",
        )?;
    } else {
        write_if_changed(
            &stage.0.join(".sdkgen/derivation.json"),
            &json_bytes(&derivation)?,
            "report.write",
        )?;
        write_if_changed(
            &stage.0.join("sdkgen.lock.json"),
            &json_bytes(&recipe)?,
            "recipe.write",
        )?;
        fs::rename(&stage.0, &output)
            .map_err(|e| err("init.publish", format!("{}: {e}", output.display())))?;
    }
    println!(
        "standalone crate: {}; reviewed recipe: sdkgen.lock.json",
        output.display()
    );
    Ok(())
}
fn main() {
    if let Err(error) = main_inner() {
        let _ = writeln!(
            io::stderr(),
            "{}",
            json!({"stage":error.stage,"message":error.message})
        );
        std::process::exit(2);
    }
}
