use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn fixture(name: &str) -> PathBuf {
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
        "rust-sdk-output-cli-{}-{nonce}",
        std::process::id()
    ))
}

fn invoke(command: &str, output: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rust-sdk-generator"))
        .args([
            command,
            "--openapi",
            fixture("openapi.json").to_str().expect("utf8 path"),
            "--bindings",
            fixture("rust-bindings.json").to_str().expect("utf8 path"),
            "--definition",
            fixture("policy.json").to_str().expect("utf8 path"),
            "--output",
            output.to_str().expect("utf8 path"),
        ])
        .output()
        .expect("run generator CLI")
}

#[test]
fn freshness_check_is_read_only_and_reports_changed_missing_and_extra_files() {
    let root = temp_dir();
    fs::create_dir(&root).expect("temp root");
    let output = root.join("sdk");
    let first = invoke("check-generated", &output);
    assert_eq!(first.status.code(), Some(1));
    let first_diff: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("JSON diff");
    assert!(first_diff["missing"].as_array().expect("missing list").len() >= 3);
    assert_eq!(first_diff["changed"], serde_json::json!([]));
    assert!(!output.exists(), "freshness check must not create output");

    let generate = invoke("generate", &output);
    assert!(generate.status.success(), "{}", String::from_utf8_lossy(&generate.stderr));
    let current = invoke("check-generated", &output);
    assert!(current.status.success(), "{}", String::from_utf8_lossy(&current.stderr));
    let clean: serde_json::Value = serde_json::from_slice(&current.stdout).expect("JSON diff");
    for key in ["missing", "changed", "extra", "conflicts"] {
        assert_eq!(clean[key], serde_json::json!([]), "{key}");
    }

    let old = fs::read_to_string(output.join("mod.rs")).expect("generated mod");
    let extra_source = old.replace("pub ", "// extra\npub ");
    fs::write(output.join("extra.rs"), extra_source).expect("extra generated file");
    fs::write(output.join("mod.rs"), format!("{old}\n// drift\n")).expect("alter generated file");
    fs::remove_file(output.join("facade_types.rs")).expect("remove generated file");
    fs::write(output.join("error.rs"), "handwritten error runtime\n").expect("handwritten file");
    let changed = fs::read(output.join("mod.rs")).expect("changed file");
    let stale = invoke("check-generated", &output);
    assert_eq!(stale.status.code(), Some(1));
    let diff: serde_json::Value = serde_json::from_slice(&stale.stdout).expect("JSON diff");
    assert_eq!(diff["missing"], serde_json::json!(["facade_types.rs"]));
    assert_eq!(diff["changed"], serde_json::json!(["mod.rs"]));
    assert_eq!(diff["extra"], serde_json::json!(["extra.rs"]));
    assert_eq!(diff["conflicts"], serde_json::json!([]));
    assert_eq!(fs::read(output.join("mod.rs")).unwrap(), changed, "check must not write");
    assert!(output.join("extra.rs").exists(), "check must not delete");

    let regenerated = invoke("generate", &output);
    assert!(regenerated.status.success(), "{}", String::from_utf8_lossy(&regenerated.stderr));
    assert!(!output.join("extra.rs").exists());
    assert_eq!(fs::read_to_string(output.join("error.rs")).unwrap(), "handwritten error runtime\n");
    assert!(invoke("check-generated", &output).status.success());
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn freshness_check_reports_handwritten_path_conflict_without_overwriting() {
    let root = temp_dir();
    fs::create_dir(&root).expect("temp root");
    let output = root.join("sdk");
    fs::create_dir(&output).expect("output");
    fs::write(output.join("mod.rs"), "handwritten mod\n").expect("handwritten path");
    let check = invoke("check-generated", &output);
    assert_eq!(check.status.code(), Some(1));
    let diff: serde_json::Value = serde_json::from_slice(&check.stdout).expect("JSON diff");
    assert_eq!(diff["conflicts"], serde_json::json!(["mod.rs"]));
    let generate = invoke("generate", &output);
    assert_eq!(generate.status.code(), Some(2));
    let diagnostic: serde_json::Value = serde_json::from_slice(&generate.stderr).expect("JSON error");
    assert_eq!(diagnostic["code"], "cli.output_conflict");
    assert_eq!(fs::read_to_string(output.join("mod.rs")).unwrap(), "handwritten mod\n");
    fs::remove_dir_all(root).expect("cleanup");
}
