//! Pinned ordinary-upstream call-shape integration proof (#155).
//! The workflow owns the immutable upstream checkout; this runner owns the
//! evidence, canonical contract, standalone build and actual HTTP test gate.

use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

type Result<T> = std::result::Result<T, String>;

fn run(label: &str, command: &mut Command) -> Result<Output> {
    println!("[{label}] {command:?}");
    let output = command
        .output()
        .map_err(|error| format!("[{label}] execute: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "[{label}] exit {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        ));
    }
    Ok(output)
}

fn command_json(label: &str, command: &mut Command) -> Result<Value> {
    let output = run(label, command)?;
    serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "[{label}] invalid JSON: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_slice(&fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?)
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn write(path: &Path, content: impl AsRef<[u8]>) -> Result<()> {
    fs::write(path, content).map_err(|error| format!("{}: {error}", path.display()))
}

fn copy_rs(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination).map_err(|error| error.to_string())?;
    for item in fs::read_dir(source).map_err(|error| error.to_string())? {
        let item = item.map_err(|error| error.to_string())?;
        let path = item.path();
        if path.extension().is_some_and(|extension| extension == "rs") {
            fs::copy(&path, destination.join(item.file_name()))
                .map_err(|error| format!("copy {}: {error}", path.display()))?;
        }
    }
    Ok(())
}

fn declared(value: &Value) -> Result<BTreeMap<(String, String), Value>> {
    let paths = value["paths"].as_object().ok_or("OpenAPI paths missing")?;
    let mut operations = BTreeMap::new();
    for (path, item) in paths {
        let entries = item.as_object().ok_or("OpenAPI path item is not an object")?;
        for (verb, operation) in entries {
            if !matches!(verb.as_str(), "get" | "post" | "put" | "patch" | "delete" | "head") {
                continue;
            }
            let responses = operation["responses"]
                .as_object()
                .ok_or("OpenAPI operation has no responses")?;
            let declared_success = responses
                .iter()
                .filter(|(status, _)| status.starts_with('2'))
                .map(|(status, response)| {
                    json!({
                        "status": status,
                        "media_types": response["content"].as_object()
                            .map(|content| content.keys().cloned().collect::<Vec<_>>())
                            .unwrap_or_default(),
                    })
                })
                .collect::<Vec<_>>();
            let request_media = operation["requestBody"]["content"]
                .as_object()
                .map(|content| content.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            let parameters = operation["parameters"].as_array().map(|parameters| {
                parameters.iter().map(|parameter| json!({
                    "name": parameter["name"],
                    "in": parameter["in"],
                    "required": parameter["required"].as_bool().unwrap_or(false),
                    "schema": parameter["schema"],
                })).collect::<Vec<_>>()
            }).unwrap_or_default();
            operations.insert(
                (verb.to_ascii_uppercase(), path.clone()),
                json!({
                    "operation_id": operation["operationId"],
                    "success": declared_success,
                    "request_media": request_media,
                    "request_schema": operation["requestBody"]["content"],
                    "parameters": parameters,
                }),
            );
        }
    }
    Ok(operations)
}

fn report(spec: &Value, semantics: &Value, bindings: &Value) -> Result<Value> {
    let declared = declared(spec)?;
    let observed = semantics["operations"].as_object().ok_or("semantic operations missing")?;
    let actual_bindings = bindings["operations"].as_object().ok_or("Bindings operations missing")?;
    let unmatched = semantics["unmatched_source_operations"].as_array().ok_or("missing unmatched list")?;
    let unsupported = semantics["unsupported_stream_methods"].as_array().ok_or("missing unsupported stream list")?;
    if !unmatched.is_empty() || !unsupported.is_empty() {
        return Err(format!(
            "[source coverage] unmatched operations: {unmatched}; unsupported streams: {unsupported}"
        ));
    }
    if bindings["schema_version"] != 3 {
        return Err("[canonical] expected Bindings v3".into());
    }
    if observed.len() != actual_bindings.len() {
        return Err(format!("[canonical] {} observed methods != {} canonical methods", observed.len(), actual_bindings.len()));
    }
    let mut matrix = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for (method, semantic) in observed {
        let source = &semantic["source_operation"];
        let verb = source["method"].as_str().ok_or("semantic HTTP verb missing")?;
        let path = source["path"].as_str().ok_or("semantic source path missing")?;
        let key = (verb.to_owned(), path.to_owned());
        let source_spec = declared.get(&key).ok_or_else(|| format!("emitted {verb} {path} absent from OpenAPI"))?;
        if semantic["source_operation"]["operation_id"] != source_spec["operation_id"] {
            return Err(format!("[source identity] {method}: source operationId mismatch"));
        }
        let canonical = actual_bindings.get(method).ok_or_else(|| format!("[canonical] {method} missing"))?;
        if canonical["metadata"]["source_operation"] != *source
            || canonical["metadata"]["emitted_operation_id"] != semantic["emitted_operation_id"]
            || canonical["metadata"]["representation"] != semantic["representation"]
            || canonical["metadata"]["success_statuses"] != semantic["success_statuses"]
        {
            return Err(format!("[semantic parity] {method}: canonical metadata != generated-method evidence"));
        }
        seen.insert(key.clone());
        matrix.entry(key).or_insert_with(Vec::new).push(json!({
            "rust_method": method,
            "raw_evidence": semantic,
            "adapter": {"proven": true, "diagnostic": null},
            "bindings_v3": canonical,
            "supported": true
        }));
    }
    if seen != declared.keys().cloned().collect() {
        let absent: Vec<_> = declared.keys().filter(|key| !seen.contains(*key)).collect();
        return Err(format!("[source coverage] un-emitted operations: {absent:?}"));
    }
    let operations = matrix.into_iter().map(|((verb, path), mut methods)| {
        methods.sort_by(|a, b| a["rust_method"].as_str().cmp(&b["rust_method"].as_str()));
        let source = declared.get(&(verb.clone(), path.clone())).expect("source");
        json!({
            "source": {"method": verb, "path": path, "openapi": source},
            "emitted": methods
        })
    }).collect::<Vec<_>>();
    Ok(json!({
        "schema_version": 1,
        "bindings_schema_version": 3,
        "backend": {
            "repository": "gpu-cli/openapi-to-rust",
            "commit": "5a3487edbe27cfd4efb32dda893774e23d7fa195",
            "config": "openapi-to-rust-bindings/tests/fixtures/call-shape-matrix/compat.toml",
            "effective_openapi": "openapi-to-rust-bindings/tests/fixtures/call-shape-matrix/openapi.json"
        },
        "operations": operations,
    }))
}

fn assemble_consumer(root: &Path, raw: &Path, sdk: &Path, work: &Path) -> Result<()> {
    let consumer = work.join("consumer");
    let source = consumer.join("src");
    let generated = source.join("generated");
    let facade = source.join("sdk");
    copy_rs(raw, &generated)?;
    copy_rs(sdk, &facade)?;
    let example = root.join("examples/independent-sdk/consumer");
    fs::copy(example.join("src/sdk/error.rs"), facade.join("error.rs"))
        .map_err(|error| format!("copy error runtime: {error}"))?;
    write(&source.join("lib.rs"), "pub mod generated;\npub mod sdk;\n")?;
    let tests = consumer.join("tests");
    fs::create_dir_all(&tests).map_err(|error| error.to_string())?;
    fs::copy(
        root.join("openapi-to-rust-bindings/tests/fixtures/call-shape-matrix/http.rs"),
        tests.join("http.rs")
    ).map_err(|error| format!("copy HTTP fixture: {error}"))?;
    let deps = fs::read_to_string(raw.join("REQUIRED_DEPS.toml"))
        .map_err(|error| format!("read backend dependencies: {error}"))?;
    if !deps.lines().any(|line| line.trim() == "[dependencies]") {
        return Err("[consumer] backend dependencies have no [dependencies] header".into());
    }
    let manifest = format!(
        "[package]\nname = \"call-shape-matrix-consumer\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n[workspace]\n\n{}\n\n[dev-dependencies]\ntokio = {{ version = \"1\", features = [\"macros\", \"rt-multi-thread\"] }}\n",
        deps.replacen("[dependencies]", "[dependencies]\nfutures-util = \"0.3\"\nbytes = \"1\"\n", 1),
    );
    write(&consumer.join("Cargo.toml"), manifest)?;
    run("independent compile and mock HTTP tests", Command::new("cargo")
        .arg("test").arg("--manifest-path").arg(consumer.join("Cargo.toml"))
        .arg("--all-targets"))?;
    println!("[consumer] independently compiled generated source and HTTP behavior passed");
    Ok(())
}

fn main_inner() -> Result<()> {
    let arguments: Vec<_> = env::args_os().skip(1).collect();
    if arguments.len() != 3 {
        return Err("usage: cargo run -p openapi-to-rust-bindings --example call_shape_matrix -- <ordinary-raw-dir> <new-empty-work-dir> <canonical-report-output>".into());
    }
    let raw = PathBuf::from(&arguments[0]).canonicalize().map_err(|error| error.to_string())?;
    let work = PathBuf::from(&arguments[1]);
    if work.exists() && fs::read_dir(&work).map_err(|error| error.to_string())?.next().is_some() {
        return Err(format!("[work dir] must be empty: {}", work.display()));
    }
    fs::create_dir_all(&work).map_err(|error| error.to_string())?;
    let work = work.canonicalize().map_err(|error| error.to_string())?;
    let output = PathBuf::from(&arguments[2]);
    if raw.join("binding-manifest.json").exists() {
        return Err("[upstream] unexpected producer binding-manifest.json".into());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().ok_or("workspace root")?.to_path_buf();
    let spec = root.join("openapi-to-rust-bindings/tests/fixtures/call-shape-matrix/openapi.json");
    let adapter = root.join("target/debug/openapi-to-rust-bindings");
    let generator = root.join("target/debug/rust-sdk-generator");
    let semantics = command_json("ordinary Rust semantic inspection", Command::new(&adapter)
        .arg("--inspect-semantics").arg(&raw).arg(&spec))?;
    let mut extracted = Vec::new();
    for attempt in 1..=3 {
        let bytes = run(&format!("deterministic Bindings extraction #{attempt}"), Command::new(&adapter)
            .arg("--extract").arg(&raw).arg(&spec))?.stdout;
        if extracted.first().is_some_and(|first| first != &bytes) {
            return Err(format!("[determinism] extraction #{attempt} differs byte-for-byte"));
        }
        extracted.push(bytes);
    }
    let bindings: Value = serde_json::from_slice(&extracted[0]).map_err(|error| error.to_string())?;
    let matrix = report(&read_json(&spec)?, &semantics, &bindings)?;
    let report_bytes = serde_json::to_vec_pretty(&matrix).map_err(|error| error.to_string())?;
    write(&output, [&report_bytes[..], b"\n"].concat())?;
    println!("[matrix] {} observed supported methods, {} distinct OpenAPI operations", bindings["operations"].as_object().map_or(0, |value| value.len()), matrix["operations"].as_array().map_or(0, Vec::len));
    let bindings_path = work.join("bindings.json");
    write(&bindings_path, &extracted[0])?;
    let definition = work.join("definition.json");
    let sdk = work.join("sdk");
    run("derive canonical Bindings", Command::new(&generator).arg("derive")
        .arg("--openapi").arg(&spec).arg("--bindings").arg(&bindings_path)
        .arg("--definition-output").arg(&definition))?;
    run("generate public facade", Command::new(&generator).arg("generate")
        .arg("--openapi").arg(&spec).arg("--bindings").arg(&bindings_path)
        .arg("--definition").arg(&definition).arg("--output").arg(&sdk))?;
    assemble_consumer(&root, &raw, &sdk, &work)?;
    Ok(())
}

fn main() {
    if let Err(error) = main_inner() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
