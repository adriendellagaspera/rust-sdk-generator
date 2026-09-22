#!/usr/bin/env python3
"""Historical fork-manifest oracle proof only; not the supported default path."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parent
OPERATIONS = {"create_note", "read_note", "delete_note", "export_note"}
GENERATED_FILES = {"mod.rs", "facade_types.rs", "notes.rs"}


def run(stage: str, *args: object, cwd: Path | None = None) -> str:
    command = [str(value) for value in args]
    result = subprocess.run(command, cwd=cwd, text=True, capture_output=True, check=False)
    if result.returncode:
        raise RuntimeError(
            f"[{stage}] command failed ({result.returncode}): {' '.join(command)}\n"
            f"{result.stdout}\n{result.stderr}"
        )
    return result.stdout


def json_file(path: Path) -> dict:
    return json.loads(path.read_text())


def snapshot(directory: Path) -> dict[str, bytes]:
    return {
        path.relative_to(directory).as_posix(): path.read_bytes()
        for path in sorted(directory.rglob("*"))
        if path.is_file()
    }


def assert_equal(stage: str, actual: object, expected: object) -> None:
    if actual != expected:
        raise AssertionError(f"[{stage}] expected {expected!r}, got {actual!r}")


def assert_report(report: dict, definition: dict) -> None:
    assert_equal("generator report", report["schema_version"], 1)
    assert_equal("generator report", set(report["operations"]), OPERATIONS)
    projected = {
        operation["operation_id"]
        for resource in definition["resources"].values()
        for operation in resource["operations"].values()
    }
    for operation_id, outcome in report["operations"].items():
        status = outcome["status"]
        reason = outcome["reason"]
        if not isinstance(reason.get("code"), str) or not reason["code"]:
            raise AssertionError(f"[generator report] missing reason for {operation_id}")
        if status in ("derived", "overridden"):
            if operation_id not in projected:
                raise AssertionError(f"[generator report] missing projection for {operation_id}")
        elif status in ("rejected", "excluded"):
            if operation_id in projected:
                raise AssertionError(f"[generator report] rejected operation projected: {operation_id}")
        else:
            raise AssertionError(f"[generator report] unknown status: {status}")
    # This independent fixture is deliberately within the existing structural
    # contract. A regression must surface the actual rejection, never silently
    # filter an operation or relax structural validation.
    if projected != OPERATIONS:
        missing = OPERATIONS - projected
        outcomes = {name: report["operations"][name] for name in sorted(missing)}
        raise AssertionError(f"[generator report] missing projections: {outcomes!r}")
    assert_equal("generator report", report["operations"]["export_note"]["status"], "overridden")
    for name in OPERATIONS - {"export_note"}:
        assert_equal("generator report", report["operations"][name]["status"], "derived")


def one_pass(backend: Path, adapter: Path, generator: Path, root: Path) -> dict:
    root.mkdir(parents=True)
    raw = root / "raw"
    raw.mkdir()
    config = root / "openapi-to-rust.toml"
    spec_path = ROOT / "openapi.json"
    config.write_text(
        "[generator]\n"
        f'spec_path = "{spec_path.as_posix()}"\n'
        f'output_dir = "{raw.as_posix()}"\n'
        'module_name = "notebook"\n'
        "binding_manifest = true\n\n"
        "[features]\n"
        "enable_async_client = true\n\n"
        "[http_client]\n"
        'base_url = "http://127.0.0.1"\n\n'
        "[http_client.retry]\n"
        "max_retries = 0\n"
    )
    run("raw backend", backend, "generate", "--config", config)
    manifest_path = raw / "binding-manifest.json"
    if not manifest_path.is_file():
        raise AssertionError("[raw backend] generator-owned manifest missing")
    manifest = json_file(manifest_path)
    assert_equal("raw backend", manifest["schema"], "openapi-to-rust.binding-manifest")
    assert_equal("raw backend", manifest["schema_version"], 1)

    bindings_path = root / "rust-bindings.json"
    bindings_path.write_text(run("bindings adapter", adapter, raw))
    bindings = json_file(bindings_path)
    assert_equal("bindings adapter", bindings["schema_version"], 3)
    identities = {
        value["metadata"]["source_operation"]["operation_id"]
        for value in bindings["operations"].values()
    }
    assert_equal("bindings adapter", identities, OPERATIONS)
    kinds = {
        value["metadata"]["representation"]["kind"]
        for value in bindings["operations"].values()
        if value["metadata"]["source_operation"]["operation_id"] == "export_note"
    }
    assert_equal("bindings adapter", kinds, {"binary_buffered", "binary_stream"})

    derive_args = (
        "--openapi", ROOT / "openapi.json",
        "--bindings", bindings_path,
        "--surface", ROOT / "surface.json",
        "--overrides", ROOT / "overrides.json",
    )
    derivation = json.loads(run("generator derive", generator, "derive", *derive_args))
    (root / "derivation.json").write_text(json.dumps(derivation, indent=2, sort_keys=True) + "\n")
    assert_report(derivation["report"], derivation["definition"])
    definition_path = root / "definition.json"
    definition_path.write_text(json.dumps(derivation["definition"], indent=2) + "\n")

    sdk = root / "sdk"
    inventory_path = root / "inventory.json"
    generation_args = (
        "--openapi", ROOT / "openapi.json",
        "--bindings", bindings_path,
        "--definition", definition_path,
        "--inventory", inventory_path,
    )
    inventory = json.loads(run(
        "generator generate", generator, "generate", *generation_args,
        "--output", sdk,
    ))
    assert_equal("generator inventory", json_file(inventory_path), inventory)
    assert_equal("generator check", json.loads(
        run("generator check", generator, "check", *generation_args)
    ), inventory)
    output = snapshot(sdk)
    assert_equal("generator files", set(output), GENERATED_FILES)
    if "pub async fn" not in output["notes.rs"].decode():
        raise AssertionError("[generator files] emitted resource has no methods")
    return {
        "manifest": manifest,
        "bindings": bindings,
        "derivation": derivation,
        "inventory": inventory,
        "generated": output,
        "raw_source": snapshot(raw),
    }


def toml_value(value: object) -> str:
    if isinstance(value, str):
        return json.dumps(value)
    if isinstance(value, bool):
        return str(value).lower()
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(item) for item in value) + "]"
    if isinstance(value, dict):
        return "{ " + ", ".join(f"{key} = {toml_value(val)}" for key, val in value.items()) + " }"
    raise TypeError(f"unexpected dependency value: {value!r}")


def consumer_manifest(raw: Path) -> str:
    requirements = tomllib.loads((raw / "REQUIRED_DEPS.toml").read_text())
    dependencies = dict(requirements["dependencies"])
    tokio = dependencies.get("tokio", {"version": "1"})
    if isinstance(tokio, str):
        tokio = {"version": tokio}
    tokio["features"] = sorted(set(tokio.get("features", [])) | {"macros", "rt-multi-thread"})
    dependencies["tokio"] = tokio
    dependencies.setdefault("futures-util", "0.3")
    dependencies.setdefault("bytes", "1")
    lines = [
        "[package]", 'name = "independent-notebook-consumer"', 'version = "0.0.0"',
        'edition = "2024"', "publish = false", "", "[workspace]", "", "[dependencies]",
    ]
    for name, value in sorted(dependencies.items()):
        lines.append(f"{name} = {toml_value(value)}")
    for target, table in sorted(requirements.get("target", {}).items()):
        lines.extend(["", f"[target.'{target}'.dependencies]"])
        for name, value in sorted(table.get("dependencies", {}).items()):
            lines.append(f"{name} = {toml_value(value)}")
    return "\n".join(lines) + "\n"


def test_consumer(root: Path) -> None:
    crate = root / "consumer"
    shutil.copytree(ROOT / "consumer", crate)
    raw = root / "first" / "raw"
    generated = crate / "src" / "generated"
    generated.mkdir()
    for path in raw.rglob("*.rs"):
        destination = generated / path.relative_to(raw)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, destination)
    sdk_dir = crate / "src" / "sdk"
    for path in (root / "first" / "sdk").rglob("*.rs"):
        destination = sdk_dir / path.relative_to(root / "first" / "sdk")
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, destination)
    (crate / "Cargo.toml").write_text(consumer_manifest(raw))
    run("consumer compile and HTTP tests", "cargo", "test", "--manifest-path",
        crate / "Cargo.toml", "--all-targets", cwd=crate)
    print("[consumer] standalone crate compiled and HTTP tests passed")
    for filename in sorted(GENERATED_FILES):
        source = (sdk_dir / filename).read_text()
        print(f"[generated Rust] {filename}: {len(source.splitlines())} lines")
    print(f"[artifacts] {root}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", type=Path, required=True)
    parser.add_argument("--adapter", type=Path, required=True)
    parser.add_argument("--generator", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    args = parser.parse_args()
    backend, adapter, generator = (
        path.resolve() for path in (args.backend, args.adapter, args.generator)
    )
    root = args.work_dir.resolve()
    root.mkdir(parents=True, exist_ok=True)
    first = one_pass(backend, adapter, generator, root / "first")
    second = one_pass(backend, adapter, generator, root / "second")
    for name in ("manifest", "bindings", "derivation", "inventory", "generated", "raw_source"):
        assert_equal(f"determinism {name}", second[name], first[name])
    print("[pipeline] pinned backend, canonical v3, exhaustive derivation, file inventory and determinism passed")
    test_consumer(root)


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, RuntimeError, ValueError, KeyError) as error:
        print(error, file=sys.stderr)
        raise SystemExit(1) from error
