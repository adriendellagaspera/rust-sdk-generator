//! Reproducible #155 capability evidence over generated Rust.
//!
//! This is fixture tooling, not a second adapter contract. It observes ordinary
//! generated Rust through the real adapter, compares the fork manifest oracle,
//! and emits one deterministic JSON report suitable for compatibility diffs.

use openapi_to_rust_bindings::{
    extract_bindings, inspect_generated, inspect_semantics, read_bindings,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, String>;

#[derive(Debug)]
struct BackendState {
    report: Value,
    bindings: Option<Value>,
    diagnostic: Option<String>,
}

#[derive(Debug)]
struct ScenarioState {
    upstream: BackendState,
    fork_oracle: BackendState,
}

fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_slice(
        &fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?,
    )
    .map_err(|error| format!("parse {}: {error}", path.display()))
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    fs::write(path, bytes).map_err(|error| format!("write {}: {error}", path.display()))
}

fn diagnostic(error: impl std::fmt::Display) -> String {
    error
        .to_string()
        .split_once(':')
        .map_or_else(|| error.to_string(), |(code, _)| code.to_owned())
}

fn compact_type(value: &str) -> String {
    let mut value: String = value.chars().filter(|ch| !ch.is_whitespace()).collect();
    while value.contains(",>") {
        value = value.replace(",>", ">");
    }
    value
}

fn normalize_oracle_types(bindings: &mut Value) {
    let Some(operations) = bindings
        .get_mut("operations")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for operation in operations.values_mut() {
        if let Some(value) = operation.get_mut("return_type")
            && let Some(text) = value.as_str()
        {
            *value = Value::String(compact_type(text));
        }
        if let Some(abi) = operation
            .get_mut("metadata")
            .and_then(|metadata| metadata.get_mut("stream_abi"))
            .and_then(Value::as_object_mut)
        {
            for key in ["native_type", "wasm_type"] {
                if let Some(value) = abi.get_mut(key)
                    && let Some(text) = value.as_str()
                {
                    *value = Value::String(compact_type(text));
                }
            }
        }
    }
}

fn first_difference(left: &Value, right: &Value, path: &str) -> Option<String> {
    if left == right {
        return None;
    }
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            let mut keys: Vec<_> = left.keys().chain(right.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                match (left.get(key), right.get(key)) {
                    (Some(left), Some(right)) => {
                        if let Some(diff) = first_difference(left, right, &format!("{path}.{key}"))
                        {
                            return Some(diff);
                        }
                    }
                    (left, right) => {
                        return Some(format!(
                            "{path}.{key}: left={} right={}",
                            left.map_or("<absent>".to_owned(), Value::to_string),
                            right.map_or("<absent>".to_owned(), Value::to_string)
                        ));
                    }
                }
            }
            None
        }
        (Value::Array(left), Value::Array(right)) if left.len() == right.len() => {
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                if let Some(diff) = first_difference(left, right, &format!("{path}[{index}]")) {
                    return Some(diff);
                }
            }
            None
        }
        _ => Some(format!("{path}: left={left} right={right}")),
    }
}

fn compare_oracle(manifest: &Value, extracted: &Value) -> Result<()> {
    let mut manifest = manifest.clone();
    let mut extracted = extracted.clone();
    normalize_oracle_types(&mut manifest);
    normalize_oracle_types(&mut extracted);
    if let Some(diff) = first_difference(&manifest, &extracted, "$") {
        return Err(format!("capability.oracle_mismatch: {diff}"));
    }
    Ok(())
}

fn extract_deterministically(raw: &Path, spec: &Path) -> Result<(Option<Value>, Option<String>)> {
    let first = extract_bindings(raw, spec);
    let second = extract_bindings(raw, spec);
    match (first, second) {
        (Ok(first), Ok(second)) => {
            let first = first.as_value().clone();
            let second = second.as_value().clone();
            let first_bytes =
                serde_json::to_vec_pretty(&first).map_err(|error| error.to_string())?;
            let second_bytes =
                serde_json::to_vec_pretty(&second).map_err(|error| error.to_string())?;
            if first_bytes != second_bytes {
                return Err(
                    "capability.nondeterministic_extraction: repeated canonical Bindings differ"
                        .to_owned(),
                );
            }
            Ok((Some(first), None))
        }
        (Err(first), Err(second)) => {
            let first_text = first.to_string();
            let second_text = second.to_string();
            if first_text != second_text {
                return Err(format!(
                    "capability.nondeterministic_extraction: repeated diagnostics differ: {first_text:?} != {second_text:?}"
                ));
            }
            Ok((None, Some(diagnostic(first))))
        }
        (first, second) => Err(format!(
            "capability.nondeterministic_extraction: repeated extraction changed outcome: first={} second={}",
            if first.is_ok() { "ok" } else { "error" },
            if second.is_ok() { "ok" } else { "error" },
        )),
    }
}

fn operation_declarations(spec: &Value) -> Result<Vec<Value>> {
    let paths = spec
        .get("paths")
        .and_then(Value::as_object)
        .ok_or("capability.invalid_fixture: OpenAPI paths must be an object")?;
    let mut declarations = Vec::new();
    for (path, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        for (verb, operation) in item {
            let method = verb.to_ascii_uppercase();
            if !matches!(
                method.as_str(),
                "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE"
            ) {
                continue;
            }
            let operation_id = operation
                .get("operationId")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!("capability.invalid_fixture: {method} {path} has no operationId")
                })?;
            let parameters = operation
                .get("parameters")
                .and_then(Value::as_array)
                .map(|parameters| {
                    parameters
                        .iter()
                        .map(|parameter| {
                            json!({
                                "name": parameter.get("name"),
                                "in": parameter.get("in"),
                                "required": parameter.get("required").cloned().unwrap_or(Value::Bool(false)),
                                "schema": parameter.get("schema").cloned().unwrap_or(Value::Null),
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let request_body = operation.get("requestBody").map(|body| {
                let mut media_types: Vec<_> = body
                    .get("content")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|content| content.keys().cloned())
                    .collect();
                media_types.sort();
                json!({
                    "required": body.get("required").cloned().unwrap_or(Value::Bool(false)),
                    "media_types": media_types,
                })
            });
            let mut responses = operation
                .get("responses")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|responses| responses.iter())
                .map(|(status, response)| {
                    let mut media_types: Vec<_> = response
                        .get("content")
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|content| content.keys().cloned())
                        .collect();
                    media_types.sort();
                    json!({"status": status, "media_types": media_types})
                })
                .collect::<Vec<_>>();
            responses.sort_by(|left, right| {
                left["status"]
                    .as_str()
                    .unwrap_or_default()
                    .cmp(right["status"].as_str().unwrap_or_default())
            });
            declarations.push(json!({
                "operation_id": operation_id,
                "method": method,
                "path": path,
                "parameters": parameters,
                "request_body": request_body.unwrap_or(Value::Null),
                "responses": responses,
            }));
        }
    }
    declarations.sort_by(|left, right| {
        (
            left["method"].as_str().unwrap_or_default(),
            left["path"].as_str().unwrap_or_default(),
        )
            .cmp(&(
                right["method"].as_str().unwrap_or_default(),
                right["path"].as_str().unwrap_or_default(),
            ))
    });
    Ok(declarations)
}

fn observe_backend(raw: &Path, spec: &Path) -> Result<BackendState> {
    let structural = inspect_generated(raw).map_err(|error| error.to_string())?;
    let structural = serde_json::to_value(structural).map_err(|error| error.to_string())?;
    let raw_methods = structural
        .pointer("/client/methods")
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let client_source = fs::read_to_string(raw.join("client.rs"))
        .map_err(|error| format!("read {}/client.rs: {error}", raw.display()))?;
    let semantic = match inspect_semantics(raw, spec) {
        Ok(value) => json!({
            "proved": true,
            "evidence": serde_json::to_value(value).map_err(|error| error.to_string())?,
            "diagnostic": null,
        }),
        Err(error) => json!({
            "proved": false,
            "evidence": null,
            "diagnostic": diagnostic(error),
        }),
    };
    let (bindings, extraction_diagnostic) = extract_deterministically(raw, spec)?;
    let adapter = json!({
        "proved": bindings.is_some(),
        "diagnostic": extraction_diagnostic,
    });
    let raw_signals = json!({
        "multipart_filename_helper": client_source.contains("multipart_filenames")
            && client_source.contains(".file_name("),
        "request_discriminator_assignment": client_source.contains("__request_discriminator_value"),
        "bytes_stream": client_source.contains(".bytes_stream()"),
    });
    let report = json!({
        "raw": {
            "methods": raw_methods,
            "signals": raw_signals,
        },
        "semantic": semantic,
        "adapter": adapter,
        "bindings": bindings,
    });
    Ok(BackendState {
        report,
        bindings,
        diagnostic: extraction_diagnostic,
    })
}

fn operation_matches(operation: &Value, operation_id: &str, selector: &Value) -> bool {
    let metadata = &operation["metadata"];
    if metadata["source_operation"]["operation_id"].as_str() != Some(operation_id) {
        return false;
    }
    if let Some(expected) = selector.get("representation").and_then(Value::as_str)
        && metadata["representation"]["kind"].as_str() != Some(expected)
    {
        return false;
    }
    if let Some(expected) = selector.get("kind").and_then(Value::as_str)
        && metadata["kind"].as_str() != Some(expected)
    {
        return false;
    }
    if selector
        .get("request_discriminator")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && metadata["request_discriminators"]
            .as_array()
            .is_none_or(Vec::is_empty)
    {
        return false;
    }
    true
}

fn capability_status(
    capability_id: &str,
    operation_id: &str,
    selector: &Value,
    backend: &BackendState,
) -> Value {
    if let Some(diagnostic) = &backend.diagnostic {
        return json!({
            "status": "adapter_evidence_gap",
            "diagnostic": diagnostic,
            "failure_owner": "adapter",
        });
    }
    let found = backend
        .bindings
        .as_ref()
        .and_then(|bindings| bindings.get("operations"))
        .and_then(Value::as_object)
        .is_some_and(|operations| {
            operations
                .values()
                .any(|operation| operation_matches(operation, operation_id, selector))
        });
    if found {
        json!({
            "status": "supported",
            "diagnostic": null,
            "failure_owner": null,
        })
    } else {
        json!({
            "status": "raw_generation_gap",
            "diagnostic": format!("raw.{capability_id}_not_emitted"),
            "failure_owner": "raw_backend",
        })
    }
}

fn assert_expected(label: &str, expected: &Value, actual: &Value) -> Result<()> {
    let expected_status = expected
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("capability.invalid_matrix: {label}.status"))?;
    let actual_status = actual
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("capability.invalid_report: {label}.status"))?;
    if expected_status != actual_status {
        return Err(format!(
            "capability.expectation_mismatch: {label}: expected status {expected_status:?}, got {actual_status:?}"
        ));
    }
    if let Some(expected_diagnostic) = expected.get("diagnostic").and_then(Value::as_str) {
        let actual_diagnostic = actual.get("diagnostic").and_then(Value::as_str);
        if actual_diagnostic != Some(expected_diagnostic) {
            return Err(format!(
                "capability.expectation_mismatch: {label}: expected diagnostic {expected_diagnostic:?}, got {actual_diagnostic:?}"
            ));
        }
    }
    Ok(())
}

fn scenario_status(backend: &BackendState) -> Value {
    if let Some(diagnostic) = &backend.diagnostic {
        json!({
            "status": "adapter_evidence_gap",
            "diagnostic": diagnostic,
            "failure_owner": "adapter",
        })
    } else {
        json!({
            "status": "supported",
            "diagnostic": null,
            "failure_owner": null,
        })
    }
}

fn compare_supported_subset(upstream: &Value, fork: &Value) -> Result<Vec<String>> {
    let upstream_operations = upstream
        .get("operations")
        .and_then(Value::as_object)
        .ok_or("capability.cross_backend_mismatch: upstream operations missing")?;
    let fork_operations = fork
        .get("operations")
        .and_then(Value::as_object)
        .ok_or("capability.cross_backend_mismatch: fork operations missing")?;
    let mut matched = Vec::new();
    for (name, upstream_operation) in upstream_operations {
        let fork_operation = fork_operations.get(name).ok_or_else(|| {
            format!(
                "capability.cross_backend_mismatch: fork oracle has no equivalent operation {name}"
            )
        })?;
        let mut upstream_operation = upstream_operation.clone();
        let mut fork_operation = fork_operation.clone();
        let mut upstream_wrapper = json!({"operations": {name: upstream_operation}});
        let mut fork_wrapper = json!({"operations": {name: fork_operation}});
        normalize_oracle_types(&mut upstream_wrapper);
        normalize_oracle_types(&mut fork_wrapper);
        upstream_operation = upstream_wrapper["operations"][name].clone();
        fork_operation = fork_wrapper["operations"][name].clone();
        if let Some(diff) = first_difference(&upstream_operation, &fork_operation, "$") {
            return Err(format!("capability.cross_backend_mismatch: {name}: {diff}"));
        }
        matched.push(name.clone());
    }
    matched.sort();
    Ok(matched)
}

fn main_inner() -> Result<()> {
    let mut matrix_path: Option<PathBuf> = None;
    let mut upstream_root: Option<PathBuf> = None;
    let mut fork_root: Option<PathBuf> = None;
    let mut output_path: Option<PathBuf> = None;
    let mut expected_path: Option<PathBuf> = None;
    let mut args = env::args_os().skip(1);
    while let Some(argument) = args.next() {
        let target = if argument == "--matrix" {
            &mut matrix_path
        } else if argument == "--upstream-root" {
            &mut upstream_root
        } else if argument == "--fork-root" {
            &mut fork_root
        } else if argument == "--output" {
            &mut output_path
        } else if argument == "--expected" {
            &mut expected_path
        } else {
            return Err(format!(
                "usage: capability-matrix --matrix FILE --upstream-root DIR --fork-root DIR --output FILE [--expected FILE]; unknown option {}",
                argument.to_string_lossy()
            ));
        };
        if target.is_some() {
            return Err(format!("duplicate option {}", argument.to_string_lossy()));
        }
        *target = Some(PathBuf::from(args.next().ok_or_else(|| {
            format!("missing value for {}", argument.to_string_lossy())
        })?));
    }

    let matrix_path = matrix_path.ok_or("missing --matrix")?;
    let upstream_root = upstream_root.ok_or("missing --upstream-root")?;
    let fork_root = fork_root.ok_or("missing --fork-root")?;
    let output_path = output_path.ok_or("missing --output")?;
    let matrix = read_json(&matrix_path)?;
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("adapter package has no repository parent")?
        .to_path_buf();

    let scenarios = matrix
        .get("scenarios")
        .and_then(Value::as_array)
        .ok_or("capability.invalid_matrix: scenarios must be an array")?;

    let mut states = BTreeMap::new();
    let mut scenario_reports = Map::new();
    for scenario in scenarios {
        let id = scenario
            .get("id")
            .and_then(Value::as_str)
            .ok_or("capability.invalid_matrix: scenario.id")?;
        let spec_path = repo_root.join(
            scenario
                .get("spec")
                .and_then(Value::as_str)
                .ok_or("capability.invalid_matrix: scenario.spec")?,
        );
        let spec = read_json(&spec_path)?;
        let declarations = operation_declarations(&spec)?;
        let upstream = observe_backend(&upstream_root.join(id).join("raw"), &spec_path)?;
        let fork = observe_backend(&fork_root.join(id).join("raw"), &spec_path)?;

        let fork_manifest = read_bindings(fork_root.join(id).join("raw"))
            .map_err(|error| format!("capability.oracle_unreadable: {id}: {error}"))?;
        let fork_extracted = fork.bindings.as_ref().ok_or_else(|| {
            format!(
                "capability.oracle_mismatch: {id}: fork extraction failed: {} (semantic: {})",
                fork.diagnostic
                    .as_deref()
                    .unwrap_or("no extraction diagnostic"),
                fork.report["semantic"]["diagnostic"]
                    .as_str()
                    .unwrap_or("none"),
            )
        })?;
        compare_oracle(fork_manifest.as_value(), fork_extracted)?;

        let upstream_status = scenario_status(&upstream);
        let fork_status = scenario_status(&fork);
        assert_expected(
            &format!("scenario.{id}.upstream"),
            &scenario["upstream"],
            &upstream_status,
        )?;
        assert_expected(
            &format!("scenario.{id}.fork_oracle"),
            &scenario["fork_oracle"],
            &fork_status,
        )?;

        let cross_backend = if scenario
            .get("cross_backend_parity")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let upstream_bindings = upstream.bindings.as_ref().ok_or_else(|| {
                format!("capability.cross_backend_mismatch: {id}: upstream extraction failed")
            })?;
            json!({
                "proved": true,
                "matched_operations": compare_supported_subset(upstream_bindings, fork_extracted)?,
            })
        } else {
            json!({"proved": false, "reason": "not equivalent or upstream shape rejected"})
        };

        scenario_reports.insert(
            id.to_owned(),
            json!({
                "openapi": declarations,
                "upstream": upstream.report,
                "fork_oracle": {
                    "observation": fork.report,
                    "manifest_match": true,
                },
                "status": {
                    "upstream": upstream_status,
                    "fork_oracle": fork_status,
                },
                "cross_backend_parity": cross_backend,
            }),
        );
        states.insert(
            id.to_owned(),
            ScenarioState {
                upstream,
                fork_oracle: fork,
            },
        );
    }

    let capabilities = matrix
        .get("capabilities")
        .and_then(Value::as_array)
        .ok_or("capability.invalid_matrix: capabilities must be an array")?;
    let mut capability_reports = Vec::new();
    for capability in capabilities {
        let id = capability
            .get("id")
            .and_then(Value::as_str)
            .ok_or("capability.invalid_matrix: capability.id")?;
        let scenario = capability
            .get("scenario")
            .and_then(Value::as_str)
            .ok_or("capability.invalid_matrix: capability.scenario")?;
        let operation_id = capability
            .get("operation_id")
            .and_then(Value::as_str)
            .ok_or("capability.invalid_matrix: capability.operation_id")?;
        let selector = capability
            .get("selector")
            .ok_or("capability.invalid_matrix: capability.selector")?;
        let state = states
            .get(scenario)
            .ok_or_else(|| format!("capability.invalid_matrix: unknown scenario {scenario:?}"))?;
        let upstream = capability_status(id, operation_id, selector, &state.upstream);
        let fork = capability_status(id, operation_id, selector, &state.fork_oracle);
        assert_expected(
            &format!("capability.{id}.upstream"),
            &capability["upstream"],
            &upstream,
        )?;
        assert_expected(
            &format!("capability.{id}.fork_oracle"),
            &capability["fork_oracle"],
            &fork,
        )?;
        capability_reports.push(json!({
            "id": id,
            "scenario": scenario,
            "operation_id": operation_id,
            "selector": selector,
            "upstream": upstream,
            "fork_oracle": fork,
        }));
    }

    let report = json!({
        "schema": "rust-sdk-generator.call-shape-capability-observation",
        "schema_version": 1,
        "fixture_version": matrix["fixture_version"],
        "bindings_schema_version": matrix["bindings_schema_version"],
        "pins": matrix["pins"],
        "scenarios": scenario_reports,
        "capabilities": capability_reports,
    });
    write_json(&output_path, &report)?;

    if let Some(expected_path) = expected_path {
        let expected = read_json(&expected_path)?;
        if let Some(diff) = first_difference(&expected, &report, "$") {
            return Err(format!("capability.report_drift: {diff}"));
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = main_inner() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oracle_comparison_normalizes_only_equivalent_rust_type_spelling() {
        let manifest = json!({
            "operations": {
                "stream": {
                    "return_type": "Result < HttpResponseByteStream , Error >",
                    "metadata": {
                        "stream_abi": {
                            "native_type": "BoxStream<'static, Result<bytes::Bytes, reqwest::Error,>>",
                            "wasm_type": "LocalBoxStream<'static, Result<bytes::Bytes, reqwest::Error,>>"
                        }
                    }
                }
            }
        });
        let extracted = json!({
            "operations": {
                "stream": {
                    "return_type": "Result<HttpResponseByteStream,Error>",
                    "metadata": {
                        "stream_abi": {
                            "native_type": "BoxStream<'static,Result<bytes::Bytes,reqwest::Error>>",
                            "wasm_type": "LocalBoxStream<'static,Result<bytes::Bytes,reqwest::Error>>"
                        }
                    }
                }
            }
        });
        compare_oracle(&manifest, &extracted).expect("equivalent type spelling");
    }

    #[test]
    fn oracle_comparison_rejects_semantic_mismatch() {
        let left = json!({
            "operations": {
                "call": {
                    "return_type": "Result<Value, Error>",
                    "metadata": {
                        "representation": {"kind": "json"},
                        "stream_abi": null
                    }
                }
            }
        });
        let right = json!({
            "operations": {
                "call": {
                    "return_type": "Result<Value, Error>",
                    "metadata": {
                        "representation": {"kind": "text"},
                        "stream_abi": null
                    }
                }
            }
        });
        let error = compare_oracle(&left, &right).expect_err("semantic mismatch");
        assert!(error.starts_with("capability.oracle_mismatch:"));
        assert!(error.contains("representation"));
    }

    #[test]
    fn difference_is_deterministic_and_path_specific() {
        let left = json!({"a": {"b": [1, 2]}});
        let right = json!({"a": {"b": [1, 3]}});
        assert_eq!(
            first_difference(&left, &right, "$").as_deref(),
            Some("$.a.b[1]: left=2 right=3")
        );
    }
}
