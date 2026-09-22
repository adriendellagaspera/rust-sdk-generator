#!/usr/bin/env python3
"""Fail-closed production compatibility at the manifest-free upstream boundary."""

from __future__ import annotations

import argparse
import difflib
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
from typing import Any


HTTP_METHODS = frozenset(
    {"get", "post", "put", "patch", "delete", "head", "options", "trace"}
)
REQUIRED_RAW = ("client.rs", "types.rs", "REQUIRED_DEPS.toml")


class StageFailure(RuntimeError):
    def __init__(self, stage: str, detail: str):
        super().__init__(detail)
        self.stage = stage
        self.detail = detail


def run(
    stage: str,
    *args: object,
    cwd: Path | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        [str(arg) for arg in args],
        cwd=cwd,
        capture_output=True,
        text=True,
        check=False,
    )
    if check and result.returncode:
        detail = result.stderr.strip() or result.stdout.strip()
        raise StageFailure(stage, f"{args!r} exited {result.returncode}: {detail}")
    return result


def parse_json(stage: str, source: str, description: str) -> dict[str, Any]:
    try:
        value = json.loads(source)
    except json.JSONDecodeError as error:
        raise StageFailure(stage, f"{description} emitted invalid JSON: {error}") from error
    if not isinstance(value, dict):
        raise StageFailure(stage, f"{description} did not emit a JSON object")
    return value


def file_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text())
    if not isinstance(value, dict):
        raise ValueError(f"{path} must contain a JSON object")
    return value


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def snapshot(root: Path) -> dict[str, bytes]:
    return {
        path.relative_to(root).as_posix(): path.read_bytes()
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def changed_files(before: dict[str, bytes], after: dict[str, bytes]) -> list[str]:
    return sorted(
        name
        for name in before.keys() | after.keys()
        if before.get(name) != after.get(name)
    )


def text_diffs(
    before: dict[str, bytes],
    after: dict[str, bytes],
    names: list[str],
    max_lines: int = 120,
) -> dict[str, list[str]]:
    output: dict[str, list[str]] = {}
    for name in names:
        left = before.get(name, b"").decode(errors="replace").splitlines()
        right = after.get(name, b"").decode(errors="replace").splitlines()
        output[name] = list(
            difflib.unified_diff(
                left,
                right,
                fromfile=f"baseline/{name}",
                tofile=f"candidate/{name}",
                lineterm="",
            )
        )[:max_lines]
    return output


def canonical_json(value: Any) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True) + "\n").encode()


def json_diff(before: Any, after: Any, max_lines: int = 160) -> list[str]:
    left = canonical_json(before).decode().splitlines()
    right = canonical_json(after).decode().splitlines()
    return list(
        difflib.unified_diff(
            left,
            right,
            fromfile="baseline",
            tofile="candidate",
            lineterm="",
        )
    )[:max_lines]


def generator_version(binary: Path) -> str:
    actual = run("configuration", binary, "--version").stdout.strip()
    match = re.fullmatch(r"openapi-to-rust\s+(.+)", actual)
    if not match:
        raise StageFailure(
            "configuration", f"unexpected generator version output: {actual!r}"
        )
    return match.group(1)


def immutable_sha(value: Any, label: str) -> str:
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{40}", value) is None:
        raise ValueError(f"{label} must be an immutable 40-character SHA")
    return value


def load_tracker(repo_root: Path, tracker_path: Path) -> dict[str, Any]:
    tracker = file_json(tracker_path)
    backend = tracker.get("backend")
    boundary = tracker.get("boundary")
    if (
        tracker.get("schema_version") != 3
        or tracker.get("bindings_schema_version") != 3
        or not isinstance(backend, dict)
        or not isinstance(boundary, dict)
        or backend.get("repository") != "gpu-cli/openapi-to-rust"
        or boundary.get("adapter_contract")
        != "ordinary_rust_plus_exact_effective_openapi"
        or boundary.get("producer_manifest_required") is not False
    ):
        raise ValueError(f"{tracker_path} is not a production manifest-free v3 tracker")
    baseline = backend.get("baseline")
    if not isinstance(baseline, dict) or not isinstance(baseline.get("version"), str):
        raise ValueError("tracker backend.baseline is invalid")
    immutable_sha(baseline.get("commit"), "tracker baseline")
    if not isinstance(backend.get("candidate_ref"), str) or not backend["candidate_ref"]:
        raise ValueError("tracker backend.candidate_ref is required")

    default = file_json(repo_root / "openapi-to-rust-bindings/DEFAULT_BACKEND.json")
    if (
        default.get("repository") != backend["repository"]
        or default.get("commit") != baseline["commit"]
        or default.get("compatibility_tracker")
        != "openapi-to-rust-bindings/COMPATIBILITY.json"
    ):
        raise ValueError(
            "COMPATIBILITY.json baseline must match DEFAULT_BACKEND.json"
        )

    for key in (
        "effective_openapi",
        "generation_config",
        "surface",
        "overrides",
        "consumer_fixture",
        "supported_envelope",
    ):
        value = boundary.get(key)
        if not isinstance(value, str) or not (repo_root / value).exists():
            raise ValueError(f"tracker boundary.{key} is missing: {value!r}")

    matrix = file_json(repo_root / boundary["supported_envelope"])
    pins = matrix.get("pins")
    upstream = pins.get("upstream") if isinstance(pins, dict) else None
    if (
        not isinstance(upstream, dict)
        or upstream.get("repository") != backend["repository"]
        or upstream.get("commit") != baseline["commit"]
    ):
        raise ValueError("supported envelope upstream pin must match production baseline")
    return tracker


def openapi_operations(spec: dict[str, Any]) -> list[dict[str, str]]:
    paths = spec.get("paths")
    if not isinstance(paths, dict):
        raise ValueError("OpenAPI paths must be an object")
    output: list[dict[str, str]] = []
    for path, item in paths.items():
        if not isinstance(item, dict):
            continue
        for method, operation in item.items():
            if method.lower() not in HTTP_METHODS:
                continue
            if not isinstance(operation, dict):
                raise ValueError(f"{method.upper()} {path} must be an object")
            operation_id = operation.get("operationId")
            if not isinstance(operation_id, str) or not operation_id:
                raise ValueError(f"{method.upper()} {path} has no operationId")
            output.append(
                {"operation_id": operation_id, "method": method.upper(), "path": str(path)}
            )
    return sorted(
        output,
        key=lambda item: (item["method"], item["path"], item["operation_id"]),
    )


def bindings_sources(bindings: dict[str, Any]) -> list[dict[str, str]]:
    operations = bindings.get("operations")
    if not isinstance(operations, dict):
        raise ValueError("Bindings v3 operations must be an object")
    output: list[dict[str, str]] = []
    for name, operation in operations.items():
        if not isinstance(operation, dict):
            raise ValueError(f"Bindings operation {name} is invalid")
        source = operation.get("metadata", {}).get("source_operation")
        if not isinstance(source, dict):
            raise ValueError(f"Bindings operation {name} has no source identity")
        identity = {
            "operation_id": source.get("operation_id"),
            "method": source.get("method"),
            "path": source.get("path"),
        }
        if not all(isinstance(value, str) and value for value in identity.values()):
            raise ValueError(f"Bindings operation {name} has incomplete source identity")
        output.append(identity)
    return sorted(
        output,
        key=lambda item: (item["method"], item["path"], item["operation_id"]),
    )


def assert_source_coverage(spec: dict[str, Any], bindings: dict[str, Any]) -> None:
    declared = openapi_operations(spec)
    observed = bindings_sources(bindings)
    if declared == observed:
        return
    declared_set = {
        (item["operation_id"], item["method"], item["path"]) for item in declared
    }
    observed_set = {
        (item["operation_id"], item["method"], item["path"]) for item in observed
    }
    raise ValueError(
        "source coverage mismatch: "
        f"unemitted={sorted(declared_set - observed_set)}; "
        f"unexpected={sorted(observed_set - declared_set)}"
    )


def diagnostic_code(stderr: str) -> str | None:
    match = re.search(r"\b(extract\.[A-Za-z0-9_.-]+)\b", stderr)
    return match.group(1) if match else None


def ensure_raw(raw: Path) -> None:
    for name in REQUIRED_RAW:
        if not (raw / name).is_file():
            raise StageFailure("raw_generation", f"missing ordinary Rust artifact: {name}")
    if (raw / "binding-manifest.json").exists():
        raise StageFailure(
            "raw_generation",
            "production upstream unexpectedly emitted binding-manifest.json",
        )


def copy_file(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)


def run_adapter(
    adapter: Path,
    raw: Path,
    spec: Path,
    *,
    allow_failure: bool = False,
) -> tuple[dict[str, Any] | None, str | None]:
    default = run("adapter_evidence", adapter, raw, spec, check=not allow_failure)
    explicit = run(
        "adapter_evidence", adapter, "--extract", raw, spec, check=not allow_failure
    )
    if default.returncode == 0 and explicit.returncode == 0:
        if default.stdout.encode() != explicit.stdout.encode():
            raise StageFailure(
                "adapter_evidence",
                "default adapter command and --extract emitted different Bindings",
            )
        bindings = parse_json(
            "adapter_evidence", default.stdout, "openapi-to-rust-bindings"
        )
        if bindings.get("schema_version") != 3:
            raise StageFailure(
                "adapter_evidence",
                f"expected Bindings v3, got {bindings.get('schema_version')!r}",
            )
        return bindings, None
    if default.returncode != explicit.returncode or default.stderr != explicit.stderr:
        raise StageFailure(
            "adapter_evidence",
            "default adapter command and --extract failed differently",
        )
    if not allow_failure:
        raise StageFailure(
            "adapter_evidence", default.stderr.strip() or "adapter extraction failed"
        )
    return None, diagnostic_code(default.stderr)


def derive_and_generate(
    root_generator: Path,
    spec: Path,
    bindings_path: Path,
    destination: Path,
    *,
    surface: Path | None = None,
    overrides: Path | None = None,
) -> tuple[dict[str, Any], dict[str, Any], dict[str, bytes]]:
    definition = destination / "definition.json"
    args: list[object] = [
        root_generator,
        "derive",
        "--openapi",
        spec,
        "--bindings",
        bindings_path,
        "--definition-output",
        definition,
    ]
    if surface is not None:
        args.extend(["--surface", surface])
    if overrides is not None:
        args.extend(["--overrides", overrides])
    derived = run("root_sdk_derivation", *args)
    derivation = parse_json(
        "root_sdk_derivation", derived.stdout, "rust-sdk-generator derive"
    )
    report = derivation.get("report")
    if not isinstance(report, dict) or not isinstance(report.get("operations"), dict):
        raise StageFailure(
            "root_sdk_derivation", "derivation has no exhaustive report.operations"
        )
    for operation, outcome in report["operations"].items():
        if not isinstance(outcome, dict) or not isinstance(outcome.get("status"), str):
            raise StageFailure(
                "root_sdk_derivation",
                f"derivation report has no status for {operation}",
            )
        reason = outcome.get("reason")
        if not isinstance(reason, dict) or not isinstance(reason.get("code"), str):
            raise StageFailure(
                "root_sdk_derivation",
                f"derivation report has no reason code for {operation}",
            )

    sdk = destination / "sdk"
    inventory_path = destination / "inventory.json"
    generated = run(
        "root_sdk_derivation",
        root_generator,
        "generate",
        "--openapi",
        spec,
        "--bindings",
        bindings_path,
        "--definition",
        definition,
        "--output",
        sdk,
        "--inventory",
        inventory_path,
    )
    inventory = parse_json(
        "root_sdk_derivation", generated.stdout, "rust-sdk-generator generate"
    )
    if file_json(inventory_path) != inventory:
        raise StageFailure(
            "root_sdk_derivation", "inventory file differs from generate stdout"
        )
    return derivation, inventory, snapshot(sdk)


def cargo_manifest(raw: Path) -> str:
    required = (raw / "REQUIRED_DEPS.toml").read_text()
    if "[dependencies]" not in required:
        raise StageFailure(
            "compiled_http", "REQUIRED_DEPS.toml has no [dependencies] section"
        )
    additions: list[str] = []
    for dependency, version in (("futures-util", "0.3"), ("bytes", "1")):
        if re.search(rf"(?m)^\\s*{re.escape(dependency)}\\s*=", required) is None:
            additions.append(f'{dependency} = "{version}"')
    if additions:
        required = required.replace(
            "[dependencies]",
            "[dependencies]\\n" + "\\n".join(additions),
            1,
        )
    return (
        '[package]\\nname = "production-compat-consumer"\\nversion = "0.0.0"\\n'
        'edition = "2024"\\npublish = false\\n\\n[workspace]\\n\\n'
        + required
        + '\\n\\n[dev-dependencies]\\ntokio = { version = "1", features = ["macros", "rt-multi-thread"] }\\n'
    )


def compile_default_consumer(
    source_fixture: Path,
    raw: Path,
    sdk: Path,
    destination: Path,
) -> None:
    shutil.copytree(source_fixture, destination)
    generated = destination / "src/generated"
    sdk_destination = destination / "src/sdk"
    generated.mkdir(parents=True, exist_ok=True)
    sdk_destination.mkdir(parents=True, exist_ok=True)
    for path in raw.glob("*.rs"):
        copy_file(path, generated / path.name)
    for path in sdk.glob("*.rs"):
        copy_file(path, sdk_destination / path.name)
    (destination / "Cargo.toml").write_text(cargo_manifest(raw))
    run(
        "compiled_http",
        "cargo",
        "test",
        "--manifest-path",
        destination / "Cargo.toml",
        "--all-targets",
        cwd=destination,
    )


def default_pass(
    repo_root: Path,
    tracker: dict[str, Any],
    generator: Path,
    adapter: Path,
    root_generator: Path,
    destination: Path,
    *,
    compile_http: bool,
) -> dict[str, Any]:
    destination.mkdir(parents=True)
    boundary = tracker["boundary"]
    spec = destination / "openapi.json"
    config = destination / "openapi-to-rust.toml"
    surface = destination / "surface.json"
    overrides = destination / "overrides.json"
    copy_file(repo_root / boundary["effective_openapi"], spec)
    copy_file(repo_root / boundary["generation_config"], config)
    copy_file(repo_root / boundary["surface"], surface)
    copy_file(repo_root / boundary["overrides"], overrides)

    run("raw_generation", generator, "generate", "--config", config.name, cwd=destination)
    raw = destination / "raw"
    ensure_raw(raw)
    raw_snapshot = snapshot(raw)

    bindings, diagnostic = run_adapter(adapter, raw, spec)
    if bindings is None or diagnostic is not None:
        raise StageFailure("adapter_evidence", diagnostic or "bindings unavailable")
    try:
        assert_source_coverage(file_json(spec), bindings)
    except ValueError as error:
        raise StageFailure("adapter_evidence", str(error)) from error
    bindings_path = destination / "rust-bindings.json"
    bindings_path.write_bytes(canonical_json(bindings))

    derivation, inventory, sdk_snapshot = derive_and_generate(
        root_generator,
        spec,
        bindings_path,
        destination,
        surface=surface,
        overrides=overrides,
    )
    rejected = {
        operation: outcome
        for operation, outcome in derivation["report"]["operations"].items()
        if outcome["status"] == "rejected"
    }
    if rejected:
        raise StageFailure(
            "root_sdk_derivation",
            f"default fixture unexpectedly rejected operations: {sorted(rejected)}",
        )

    compiled_http = "not_run"
    if compile_http:
        compile_default_consumer(
            repo_root / boundary["consumer_fixture"],
            raw,
            destination / "sdk",
            destination / "consumer",
        )
        compiled_http = "passed"

    return {
        "provenance": {
            "effective_openapi_sha256": sha256_file(spec),
            "generation_config_sha256": sha256_file(config),
            "surface_sha256": sha256_file(surface),
            "overrides_sha256": sha256_file(overrides),
        },
        "source_operations": openapi_operations(file_json(spec)),
        "raw": raw_snapshot,
        "bindings": bindings,
        "derivation": derivation,
        "inventory": inventory,
        "sdk": sdk_snapshot,
        "compiled_http": compiled_http,
    }


def selector_matches(
    operation: dict[str, Any],
    operation_id: str,
    selector: dict[str, Any],
) -> bool:
    metadata = operation.get("metadata")
    if not isinstance(metadata, dict):
        return False
    source = metadata.get("source_operation")
    if not isinstance(source, dict) or source.get("operation_id") != operation_id:
        return False
    if "representation" in selector:
        representation = metadata.get("representation")
        if (
            not isinstance(representation, dict)
            or representation.get("kind") != selector["representation"]
        ):
            return False
    if "kind" in selector and metadata.get("kind") != selector["kind"]:
        return False
    if selector.get("request_discriminator") is True:
        values = metadata.get("request_discriminators")
        if not isinstance(values, list) or not values:
            return False
    return True


def expected_status(expected: Any, label: str) -> tuple[str, str | None]:
    if not isinstance(expected, dict) or not isinstance(expected.get("status"), str):
        raise ValueError(f"{label} expected status is invalid")
    diagnostic = expected.get("diagnostic")
    if diagnostic is not None and not isinstance(diagnostic, str):
        raise ValueError(f"{label} expected diagnostic is invalid")
    return expected["status"], diagnostic


def capability_observation(
    bindings: dict[str, Any] | None,
    extraction_diagnostic: str | None,
    capability: dict[str, Any],
) -> dict[str, Any]:
    if extraction_diagnostic is not None:
        return {
            "status": "adapter_evidence_gap",
            "diagnostic": extraction_diagnostic,
            "failure_owner": "adapter",
        }
    if bindings is None:
        raise ValueError("bindings missing without extraction diagnostic")
    operations = bindings.get("operations")
    if not isinstance(operations, dict):
        raise ValueError("Bindings operations missing")
    found = any(
        isinstance(operation, dict)
        and selector_matches(
            operation, capability["operation_id"], capability["selector"]
        )
        for operation in operations.values()
    )
    if found:
        return {"status": "supported", "diagnostic": None, "failure_owner": None}
    return {
        "status": "raw_generation_gap",
        "diagnostic": f"raw.{capability['id']}_not_emitted",
        "failure_owner": "raw_backend",
    }


def check_expected(actual: dict[str, Any], expected: Any, label: str) -> None:
    status, diagnostic = expected_status(expected, label)
    if actual.get("status") != status:
        raise StageFailure(
            "supported_envelope",
            f"{label}: expected status {status!r}, got {actual.get('status')!r}",
        )
    if diagnostic is not None and actual.get("diagnostic") != diagnostic:
        raise StageFailure(
            "supported_envelope",
            f"{label}: expected diagnostic {diagnostic!r}, got {actual.get('diagnostic')!r}",
        )


def compile_capability_core(
    repo_root: Path,
    raw: Path,
    sdk: Path,
    destination: Path,
) -> None:
    fixture_root = repo_root / "openapi-to-rust-bindings/tests/fixtures/capability-v1"
    (destination / "src/generated").mkdir(parents=True)
    (destination / "src/sdk").mkdir(parents=True)
    (destination / "tests").mkdir(parents=True)
    for path in raw.glob("*.rs"):
        copy_file(path, destination / "src/generated" / path.name)
    for path in sdk.glob("*.rs"):
        copy_file(path, destination / "src/sdk" / path.name)
    copy_file(
        repo_root / "examples/independent-sdk/consumer/src/sdk/error.rs",
        destination / "src/sdk/error.rs",
    )
    copy_file(fixture_root / "core/http.rs", destination / "tests/http.rs")
    (destination / "src/lib.rs").write_text("pub mod generated;\\npub mod sdk;\\n")
    (destination / "Cargo.toml").write_text(cargo_manifest(raw))
    run(
        "compiled_http",
        "cargo",
        "test",
        "--manifest-path",
        destination / "Cargo.toml",
        "--all-targets",
        cwd=destination,
    )


def envelope_pass(
    repo_root: Path,
    tracker: dict[str, Any],
    generator: Path,
    adapter: Path,
    root_generator: Path,
    destination: Path,
    *,
    compile_http: bool,
) -> dict[str, Any]:
    matrix = file_json(repo_root / tracker["boundary"]["supported_envelope"])
    scenarios = matrix.get("scenarios")
    capabilities = matrix.get("capabilities")
    if not isinstance(scenarios, list) or not isinstance(capabilities, list):
        raise StageFailure("supported_envelope", "capability matrix is invalid")
    fixture_root = repo_root / "openapi-to-rust-bindings/tests/fixtures/capability-v1"
    results: dict[str, Any] = {}
    for scenario in scenarios:
        if not isinstance(scenario, dict) or not isinstance(scenario.get("id"), str):
            raise StageFailure("supported_envelope", "scenario entry is invalid")
        scenario_id = scenario["id"]
        scenario_dir = destination / scenario_id
        scenario_dir.mkdir(parents=True)
        spec = scenario_dir / "openapi.json"
        config = scenario_dir / "config.toml"
        copy_file(repo_root / scenario["spec"], spec)
        copy_file(fixture_root / "upstream.toml", config)
        run(
            "raw_generation",
            generator,
            "generate",
            "--config",
            config.name,
            cwd=scenario_dir,
        )
        raw = scenario_dir / "raw"
        ensure_raw(raw)
        bindings, extraction_diagnostic = run_adapter(
            adapter, raw, spec, allow_failure=True
        )

        actual_scenario = (
            {"status": "supported", "diagnostic": None, "failure_owner": None}
            if extraction_diagnostic is None
            else {
                "status": "adapter_evidence_gap",
                "diagnostic": extraction_diagnostic,
                "failure_owner": "adapter",
            }
        )
        check_expected(
            actual_scenario,
            scenario["upstream"],
            f"scenario.{scenario_id}.upstream",
        )
        expected_scenario_status, expected_scenario_diagnostic = expected_status(
            scenario["upstream"], f"scenario.{scenario_id}.upstream"
        )
        if expected_scenario_status == "supported":
            if bindings is None:
                raise StageFailure(
                    "adapter_evidence",
                    f"{scenario_id}: expected Bindings but extraction failed",
                )
            try:
                assert_source_coverage(file_json(spec), bindings)
            except ValueError as error:
                raise StageFailure(
                    "adapter_evidence", f"{scenario_id}: {error}"
                ) from error
        elif extraction_diagnostic != expected_scenario_diagnostic:
            raise StageFailure(
                "adapter_evidence",
                f"{scenario_id}: expected {expected_scenario_diagnostic}, "
                f"got {extraction_diagnostic}",
            )

        capability_results: list[dict[str, Any]] = []
        for capability in capabilities:
            if not isinstance(capability, dict) or capability.get("scenario") != scenario_id:
                continue
            actual = capability_observation(
                bindings, extraction_diagnostic, capability
            )
            check_expected(
                actual,
                capability["upstream"],
                f"capability.{capability['id']}.upstream",
            )
            capability_results.append({"id": capability["id"], **actual})

        derivation: dict[str, Any] | None = None
        inventory: dict[str, Any] | None = None
        sdk_snapshot: dict[str, bytes] | None = None
        compiled = "not_run"
        if bindings is not None:
            bindings_path = scenario_dir / "rust-bindings.json"
            bindings_path.write_bytes(canonical_json(bindings))
            derivation, inventory, sdk_snapshot = derive_and_generate(
                root_generator, spec, bindings_path, scenario_dir
            )
            if scenario_id == "core" and compile_http:
                compile_capability_core(
                    repo_root,
                    raw,
                    scenario_dir / "sdk",
                    scenario_dir / "consumer",
                )
                compiled = "passed"

        results[scenario_id] = {
            "provenance": {
                "effective_openapi_sha256": sha256_file(spec),
                "generation_config_sha256": sha256_file(config),
            },
            "scenario_status": actual_scenario,
            "capabilities": capability_results,
            "raw": snapshot(raw),
            "bindings": bindings,
            "extraction_diagnostic": extraction_diagnostic,
            "derivation": derivation,
            "inventory": inventory,
            "sdk": sdk_snapshot,
            "compiled_http": compiled,
        }
    return results


def compare_snapshots(
    before: dict[str, bytes],
    after: dict[str, bytes],
) -> dict[str, Any]:
    changed = changed_files(before, after)
    return {"changed": changed, "diffs": text_diffs(before, after, changed)}


def compare_default(
    baseline: dict[str, Any],
    candidate: dict[str, Any],
) -> dict[str, Any]:
    raw = compare_snapshots(baseline["raw"], candidate["raw"])
    sdk = compare_snapshots(baseline["sdk"], candidate["sdk"])
    bindings_changed = baseline["bindings"] != candidate["bindings"]
    derivation_changed = baseline["derivation"] != candidate["derivation"]
    inventory_changed = baseline["inventory"] != candidate["inventory"]
    source_changed = baseline["source_operations"] != candidate["source_operations"]
    return {
        "source_api_changed": source_changed,
        "source_api_diff": json_diff(
            baseline["source_operations"], candidate["source_operations"]
        )
        if source_changed
        else [],
        "raw": raw,
        "bindings_changed": bindings_changed,
        "bindings_diff": json_diff(baseline["bindings"], candidate["bindings"])
        if bindings_changed
        else [],
        "derivation_changed": derivation_changed,
        "derivation_diff": json_diff(
            baseline["derivation"], candidate["derivation"]
        )
        if derivation_changed
        else [],
        "inventory_changed": inventory_changed,
        "inventory_diff": json_diff(
            baseline["inventory"], candidate["inventory"]
        )
        if inventory_changed
        else [],
        "sdk": sdk,
        "compatible": not (
            source_changed
            or raw["changed"]
            or bindings_changed
            or derivation_changed
            or inventory_changed
            or sdk["changed"]
        ),
    }


def compare_envelope(
    baseline: dict[str, Any],
    candidate: dict[str, Any],
) -> dict[str, Any]:
    scenarios: dict[str, Any] = {}
    compatible = True
    for name in sorted(baseline.keys() | candidate.keys()):
        if name not in baseline or name not in candidate:
            scenarios[name] = {"missing": True, "compatible": False}
            compatible = False
            continue
        before, after = baseline[name], candidate[name]
        raw = compare_snapshots(before["raw"], after["raw"])
        bindings_changed = before["bindings"] != after["bindings"]
        diagnostic_changed = (
            before["extraction_diagnostic"] != after["extraction_diagnostic"]
        )
        derivation_changed = before["derivation"] != after["derivation"]
        inventory_changed = before["inventory"] != after["inventory"]
        sdk = compare_snapshots(before["sdk"] or {}, after["sdk"] or {})
        row_compatible = not (
            raw["changed"]
            or bindings_changed
            or diagnostic_changed
            or derivation_changed
            or inventory_changed
            or sdk["changed"]
        )
        compatible &= row_compatible
        scenarios[name] = {
            "compatible": row_compatible,
            "raw": raw,
            "bindings_changed": bindings_changed,
            "bindings_diff": json_diff(before["bindings"], after["bindings"])
            if bindings_changed
            else [],
            "diagnostic_changed": diagnostic_changed,
            "baseline_diagnostic": before["extraction_diagnostic"],
            "candidate_diagnostic": after["extraction_diagnostic"],
            "derivation_changed": derivation_changed,
            "derivation_diff": json_diff(
                before["derivation"], after["derivation"]
            )
            if derivation_changed
            else [],
            "inventory_changed": inventory_changed,
            "inventory_diff": json_diff(
                before["inventory"], after["inventory"]
            )
            if inventory_changed
            else [],
            "sdk": sdk,
        }
    return {"compatible": compatible, "scenarios": scenarios}


def public_pass(value: dict[str, Any]) -> dict[str, Any]:
    return {
        key: item
        for key, item in value.items()
        if key not in {"raw", "sdk"}
    }


def public_envelope(value: dict[str, Any]) -> dict[str, Any]:
    return {
        name: {
            key: item
            for key, item in scenario.items()
            if key not in {"raw", "sdk"}
        }
        for name, scenario in value.items()
    }


def markdown(report: dict[str, Any]) -> str:
    backend = report.get("backend", {})
    baseline = backend.get("baseline", {})
    candidate = backend.get("candidate", {})
    default = report.get("default_boundary", {})
    envelope = report.get("supported_envelope", {})
    return "\\n".join(
        [
            "# Production manifest-free compatibility",
            "",
            f"- Repository: {backend.get('repository', 'unavailable')}",
            f"- Baseline: {baseline.get('version', 'unavailable')} / {baseline.get('commit', 'unavailable')}",
            f"- Candidate: {candidate.get('version', 'unavailable')} / {candidate.get('commit', 'unavailable')}",
            "- Contract: unmodified upstream ordinary Rust + exact effective OpenAPI + default adapter",
            "- Producer manifest required: no",
            f"- Last stage: {report['last_stage']}",
            f"- Default boundary drift-free: {default.get('compatible', False)}",
            f"- Supported envelope drift-free: {envelope.get('compatible', False)}",
            f"- Candidate default compiled/mock-HTTP proof: {report.get('candidate_compiled_http', 'not_run')}",
            f"- Candidate capability-core compiled/mock-HTTP proof: {report.get('candidate_capability_http', 'not_run')}",
            f"- Result: {'compatible' if report['compatible'] else 'incompatible'}",
            f"- Diagnostic: {report.get('diagnostic', 'none')}",
            "",
            "Any raw, Bindings, derivation, inventory or generated SDK drift is fail-closed and must be reviewed before repinning.",
        ]
    ) + "\\n"
