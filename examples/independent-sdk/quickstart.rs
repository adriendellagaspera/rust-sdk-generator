//! Native, pinned, local-first onboarding proof for an independent Rust SDK.
//! This is a fixture-specific example, not the generic init/sync product CLI.

use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

type ProofResult<T> = Result<T, String>;

const OPERATIONS: [&str; 4] = ["create_note", "delete_note", "export_note", "read_note"];
const SDK_FILES: [&str; 3] = ["facade_types.rs", "mod.rs", "notes.rs"];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn example() -> PathBuf {
    root().join("examples/independent-sdk")
}

fn read(path: &Path) -> ProofResult<Vec<u8>> {
    fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))
}

fn json_file(path: &Path) -> ProofResult<Value> {
    serde_json::from_slice(&read(path)?)
        .map_err(|error| format!("parse {}: {error}", path.display()))
}

fn write(path: &Path, source: impl AsRef<[u8]>) -> ProofResult<()> {
    fs::write(path, source).map_err(|error| format!("write {}: {error}", path.display()))
}

fn mkdir(path: &Path) -> ProofResult<()> {
    fs::create_dir_all(path).map_err(|error| format!("create {}: {error}", path.display()))
}

fn run(stage: &str, command: &mut Command) -> ProofResult<Output> {
    println!("[{stage}] {command:?}");
    let result = command
        .output()
        .map_err(|error| format!("[{stage}] execute {command:?}: {error}"))?;
    if !result.status.success() {
        return Err(format!(
            "[{stage}] {command:?} exited {}\nstdout:\n{}\nstderr:\n{}",
            result.status,
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr),
        ));
    }
    Ok(result)
}

fn stdout_json(stage: &str, command: &mut Command) -> ProofResult<Value> {
    let result = run(stage, command)?;
    serde_json::from_slice(&result.stdout).map_err(|error| {
        format!(
            "[{stage}] invalid JSON stdout: {error}\n{}",
            String::from_utf8_lossy(&result.stdout)
        )
    })
}

fn exact_revision(backend: &Path, expected: &str) -> ProofResult<()> {
    let result = run(
        "backend pin",
        Command::new("git")
            .args(["-C"])
            .arg(backend)
            .args(["rev-parse", "HEAD"]),
    )?;
    let found = String::from_utf8_lossy(&result.stdout);
    if found.trim() != expected {
        return Err(format!(
            "[backend pin] expected {expected}, found {}",
            found.trim()
        ));
    }
    Ok(())
}

fn assert_equal<T: std::fmt::Debug + PartialEq>(
    stage: &str,
    actual: &T,
    expected: &T,
) -> ProofResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("[{stage}] expected {expected:?}, found {actual:?}"))
    }
}

fn snapshot(path: &Path) -> ProofResult<BTreeMap<PathBuf, Vec<u8>>> {
    fn visit(
        root: &Path,
        current: &Path,
        output: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) -> ProofResult<()> {
        for entry in
            fs::read_dir(current).map_err(|error| format!("read {}: {error}", current.display()))?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_dir() {
                visit(root, &path, output)?;
            } else if kind.is_file() {
                output.insert(
                    path.strip_prefix(root).expect("under root").to_owned(),
                    read(&path)?,
                );
            } else {
                return Err(format!(
                    "unsupported symlink or special file {}",
                    path.display()
                ));
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(path, path, &mut files)?;
    Ok(files)
}

fn copy_tree(source: &Path, destination: &Path, rust_only: bool) -> ProofResult<()> {
    mkdir(destination)?;
    for entry in
        fs::read_dir(source).map_err(|error| format!("read {}: {error}", source.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let target = destination.join(entry.file_name());
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_dir() {
            copy_tree(&path, &target, rust_only)?;
        } else if kind.is_file() {
            if !rust_only || path.extension() == Some(OsStr::new("rs")) {
                fs::copy(&path, &target)
                    .map_err(|error| format!("copy {}: {error}", path.display()))?;
            }
        } else {
            return Err(format!(
                "unsupported symlink or special file {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn assert_report(derivation: &Value) -> ProofResult<()> {
    let report = &derivation["report"];
    assert_equal("derivation version", &report["schema_version"], &json!(1))?;
    let operations = report["operations"]
        .as_object()
        .ok_or("[generator report] missing report.operations")?;
    let expected: BTreeSet<_> = OPERATIONS.iter().copied().collect();
    let actual: BTreeSet<_> = operations.keys().map(String::as_str).collect();
    assert_equal("generator report operations", &actual, &expected)?;

    let projected: BTreeSet<_> = derivation["definition"]["resources"]
        .as_object()
        .ok_or("[generator report] missing definition.resources")?
        .values()
        .flat_map(|resource| {
            resource["operations"]
                .as_object()
                .into_iter()
                .flat_map(|ops| ops.values())
        })
        .filter_map(|operation| operation["operation_id"].as_str())
        .collect();
    assert_equal("generator projected operations", &projected, &expected)?;
    for (operation, outcome) in operations {
        let status = outcome["status"]
            .as_str()
            .ok_or_else(|| format!("[generator report] missing status for {operation}"))?;
        let code = outcome["reason"]["code"]
            .as_str()
            .filter(|code| !code.is_empty())
            .ok_or_else(|| format!("[generator report] missing reason code for {operation}"))?;
        let expected_status = if operation == "export_note" {
            "overridden"
        } else {
            "derived"
        };
        if status != expected_status {
            return Err(format!(
                "[generator report] operation {operation}: {status} ({code}): {}",
                outcome["reason"]["detail"]
            ));
        }
    }
    Ok(())
}

#[derive(PartialEq, Debug)]
struct Pass {
    manifest: Value,
    bindings: Value,
    derivation: Value,
    inventory: Value,
    raw: BTreeMap<PathBuf, Vec<u8>>,
    sdk: BTreeMap<PathBuf, Vec<u8>>,
}

fn pass(backend: &Path, adapter: &Path, generator: &Path, destination: &Path) -> ProofResult<Pass> {
    mkdir(destination)?;
    let raw = destination.join("raw");
    mkdir(&raw)?;
    let config = destination.join("openapi-to-rust.toml");
    let spec = example().join("openapi.json");
    let spec_toml = serde_json::to_string(&spec.to_string_lossy().as_ref())
        .map_err(|error| error.to_string())?;
    let raw_toml = serde_json::to_string(&raw.to_string_lossy().as_ref())
        .map_err(|error| error.to_string())?;
    write(
        &config,
        format!(
            "[generator]\nspec_path = {spec_toml}\noutput_dir = {raw_toml}\nmodule_name = \"notebook\"\nbinding_manifest = true\n\n[features]\nenable_async_client = true\n\n[http_client]\nbase_url = \"http://127.0.0.1\"\n\n[http_client.retry]\nmax_retries = 0\n"
        ),
    )?;
    run(
        "raw backend",
        Command::new(backend)
            .args(["generate", "--config"])
            .arg(&config),
    )?;
    let manifest_path = raw.join("binding-manifest.json");
    let manifest = json_file(&manifest_path)?;
    assert_equal(
        "raw backend manifest",
        &manifest["schema"],
        &json!("openapi-to-rust.binding-manifest"),
    )?;
    assert_equal(
        "raw backend manifest version",
        &manifest["schema_version"],
        &json!(1),
    )?;
    let bindings_path = destination.join("rust-bindings.json");
    let adapted = run("bindings adapter", Command::new(adapter).arg(&raw))?;
    write(&bindings_path, adapted.stdout)?;
    let bindings = json_file(&bindings_path)?;
    assert_equal("bindings version", &bindings["schema_version"], &json!(3))?;

    let found: BTreeSet<_> = bindings["operations"]
        .as_object()
        .ok_or("[bindings adapter] missing operation records")?
        .values()
        .filter_map(|binding| binding["metadata"]["source_operation"]["operation_id"].as_str())
        .collect();
    let expected: BTreeSet<_> = OPERATIONS.into_iter().collect();
    assert_equal("bindings source operation identities", &found, &expected)?;

    let derivation = stdout_json(
        "generator derive",
        Command::new(generator)
            .arg("derive")
            .args(["--openapi"])
            .arg(&spec)
            .args(["--bindings"])
            .arg(&bindings_path)
            .args(["--surface"])
            .arg(example().join("surface.json"))
            .args(["--overrides"])
            .arg(example().join("overrides.json")),
    )?;
    assert_report(&derivation)?;
    write(
        &destination.join("derivation.json"),
        serde_json::to_vec_pretty(&derivation).map_err(|error| error.to_string())?,
    )?;
    let definition_path = destination.join("definition.json");
    write(
        &definition_path,
        serde_json::to_vec_pretty(&derivation["definition"]).map_err(|error| error.to_string())?,
    )?;
    let generated = destination.join("sdk");
    let inventory_path = destination.join("inventory.json");
    let args = |kind: &str| {
        let mut command = Command::new(generator);
        command
            .arg(kind)
            .args(["--openapi"])
            .arg(&spec)
            .args(["--bindings"])
            .arg(&bindings_path)
            .args(["--definition"])
            .arg(&definition_path);
        command
    };
    let inventory = stdout_json(
        "generator generate",
        args("generate")
            .args(["--output"])
            .arg(&generated)
            .args(["--inventory"])
            .arg(&inventory_path),
    )?;
    assert_equal("API inventory", &json_file(&inventory_path)?, &inventory)?;
    assert_equal(
        "generator in-memory check",
        &stdout_json("generator check", &mut args("check"))?,
        &inventory,
    )?;
    let sdk = snapshot(&generated)?;
    let actual_names: BTreeSet<_> = sdk
        .keys()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let expected_names: BTreeSet<_> = SDK_FILES.iter().map(|name| (*name).to_owned()).collect();
    assert_equal("generated SDK files", &actual_names, &expected_names)?;
    let freshness = stdout_json(
        "generator freshness",
        args("check-generated").args(["--output"]).arg(&generated),
    )?;
    assert_equal(
        "generated SDK freshness",
        &freshness,
        &json!({"missing":[],"changed":[],"extra":[],"conflicts":[]}),
    )?;
    assert_equal("read-only freshness", &snapshot(&generated)?, &sdk)?;

    Ok(Pass {
        manifest,
        bindings,
        derivation,
        inventory,
        raw: snapshot(&raw)?,
        sdk,
    })
}

fn requirements(raw: &Path) -> ProofResult<String> {
    let required_path = raw.join("REQUIRED_DEPS.toml");
    let source = String::from_utf8(read(&required_path)?)
        .map_err(|error| format!("{}: {error}", required_path.display()))?;
    if !source.lines().any(|line| line.trim() == "[dependencies]") {
        return Err(format!(
            "[consumer manifest] {} has no [dependencies] section",
            required_path.display()
        ));
    }
    let has = |dependency: &str| {
        source.lines().any(|line| {
            line.trim_start()
                .strip_prefix(dependency)
                .is_some_and(|tail| tail.trim_start().starts_with('='))
        })
    };
    let mut add = String::new();
    if !has("futures-util") {
        add.push_str("futures-util = \"0.3\"\n");
    }
    if !has("bytes") {
        add.push_str("bytes = \"1\"\n");
    }
    let source = source.replacen("[dependencies]", &format!("[dependencies]\n{add}"), 1);
    Ok(format!(
        "[package]\nname = \"independent-notebook-consumer\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[workspace]\n\n{source}\n\n[dev-dependencies]\ntokio = {{ version = \"1\", features = [\"macros\", \"rt-multi-thread\"] }}\n"
    ))
}

fn consumer(work: &Path) -> ProofResult<()> {
    let crate_dir = work.join("consumer");
    copy_tree(&example().join("consumer"), &crate_dir, false)?;
    let raw = work.join("first/raw");
    copy_tree(&raw, &crate_dir.join("src/generated"), true)?;
    copy_tree(&work.join("first/sdk"), &crate_dir.join("src/sdk"), true)?;
    write(&crate_dir.join("Cargo.toml"), requirements(&raw)?)?;
    run(
        "consumer compile and HTTP tests",
        Command::new("cargo")
            .args(["test", "--manifest-path"])
            .arg(crate_dir.join("Cargo.toml"))
            .arg("--all-targets")
            .current_dir(&crate_dir),
    )?;
    println!("[consumer] standalone crate compiled and HTTP tests passed");
    Ok(())
}

fn main_inner() -> ProofResult<()> {
    let mut arguments = env::args_os().skip(1);
    let work = match arguments.next() {
        None => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos();
            env::temp_dir().join(format!("rust-sdk-quickstart-{}-{now}", std::process::id()))
        }
        Some(flag) if flag == "--help" || flag == "-h" => {
            println!(
                "usage: cargo run --locked --example independent-sdk-quickstart -- [--work-dir NEW_OR_EMPTY_DIR]"
            );
            return Ok(());
        }
        Some(flag) if flag == "--work-dir" => {
            let path = PathBuf::from(arguments.next().ok_or("missing path for --work-dir")?);
            if arguments.next().is_some() {
                return Err("unexpected arguments after --work-dir".to_owned());
            }
            path
        }
        Some(flag) => return Err(format!("unsupported option: {}", flag.to_string_lossy())),
    };
    if work.exists()
        && fs::read_dir(&work)
            .map_err(|error| error.to_string())?
            .next()
            .is_some()
    {
        return Err(format!(
            "working directory must be empty: {}",
            work.display()
        ));
    }
    mkdir(&work)?;
    let work = work.canonicalize().map_err(|error| error.to_string())?;
    println!("[quickstart] artifacts: {}", work.display());

    let compatibility = json_file(&root().join("openapi-to-rust-bindings/COMPATIBILITY.json"))?;
    let pin = compatibility["backend"]["baseline"]["commit"]
        .as_str()
        .filter(|value| value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("[backend pin] invalid immutable backend SHA in COMPATIBILITY.json")?;
    let backend = work.join("_backend");
    run(
        "backend checkout",
        Command::new("git").arg("init").arg(&backend),
    )?;
    run(
        "backend checkout",
        Command::new("git").args(["-C"]).arg(&backend).args([
            "remote",
            "add",
            "origin",
            "https://github.com/adriendellagaspera/openapi-to-rust.git",
        ]),
    )?;
    run(
        "backend checkout",
        Command::new("git")
            .args(["-C"])
            .arg(&backend)
            .args(["fetch", "--depth=1", "origin"])
            .arg(pin),
    )?;
    run(
        "backend checkout",
        Command::new("git")
            .args(["-C"])
            .arg(&backend)
            .args(["checkout", "--detach", "FETCH_HEAD"]),
    )?;
    exact_revision(&backend, pin)?;

    let backend_target = work.join("_backend-target");
    run(
        "backend build",
        Command::new("cargo")
            .args(["build", "--locked", "--release", "--manifest-path"])
            .arg(backend.join("Cargo.toml"))
            .args(["--bin", "openapi-to-rust"])
            .env("CARGO_TARGET_DIR", &backend_target),
    )?;
    let tools_target = work.join("_tools-target");
    for package in ["openapi-to-rust-bindings", "rust-sdk-generator"] {
        run(
            &format!("{package} build"),
            Command::new("cargo")
                .args(["build", "--locked", "--manifest-path"])
                .arg(root().join("Cargo.toml"))
                .args(["-p", package, "--bin", package])
                .env("CARGO_TARGET_DIR", &tools_target),
        )?;
    }
    let first = pass(
        &backend_target.join("release/openapi-to-rust"),
        &tools_target.join("debug/openapi-to-rust-bindings"),
        &tools_target.join("debug/rust-sdk-generator"),
        &work.join("first"),
    )?;
    let second = pass(
        &backend_target.join("release/openapi-to-rust"),
        &tools_target.join("debug/openapi-to-rust-bindings"),
        &tools_target.join("debug/rust-sdk-generator"),
        &work.join("second"),
    )?;
    assert_equal("two independent complete generations", &second, &first)?;
    println!(
        "[pipeline] pinned backend, Bindings v3, derivation and byte-level determinism passed"
    );
    consumer(&work)?;
    println!(
        "[quickstart] complete: inspect {} and {}",
        work.join("first").display(),
        work.join("consumer").display()
    );
    Ok(())
}

fn main() {
    if let Err(error) = main_inner() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
