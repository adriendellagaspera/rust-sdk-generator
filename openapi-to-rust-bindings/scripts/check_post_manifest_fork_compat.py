#!/usr/bin/env python3
"""Fail-closed fork-main compatibility gate after producer-manifest removal.

This is a transitional, *fork* raw-generation watch, not the future upstream
default or #157's production/nightly migration. A pinned historical producer
manifest is an oracle only; the candidate must expose ordinary generated Rust.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
from typing import Any


SCENARIOS = ("core", "sse", "discriminator")
TYPE_FIELDS = ("return_type",)
STREAM_TYPE_FIELDS = ("native_type", "wasm_type")
SKIPPED_RAW_FILES = frozenset({"binding-manifest.json"})


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def normalized_type(value: str) -> str:
    """Normalize only Rust type whitespace and trailing generic commas."""
    return re.sub(r",(?=>)", "", re.sub(r"\s+", "", value))


def normalized_bindings(value: dict[str, Any]) -> dict[str, Any]:
    if value.get("schema_version") != 3 or not isinstance(value.get("operations"), dict):
        raise ValueError("canonical Bindings v3 with an operations object is required")
    value = json.loads(json.dumps(value))
    for operation in value["operations"].values():
        for field in TYPE_FIELDS:
            if isinstance(operation.get(field), str):
                operation[field] = normalized_type(operation[field])
        abi = operation.get("metadata", {}).get("stream_abi")
        if isinstance(abi, dict):
            for field in STREAM_TYPE_FIELDS:
                if isinstance(abi.get(field), str):
                    abi[field] = normalized_type(abi[field])
    return value


def expected_sources(spec: dict[str, Any]) -> set[tuple[str, str, str]]:
    paths = spec.get("paths")
    if not isinstance(paths, dict):
        raise ValueError("effective OpenAPI paths must be an object")
    sources = set()
    for path, item in paths.items():
        if not isinstance(item, dict):
            raise ValueError(f"invalid OpenAPI path item {path}")
        for verb, op in item.items():
            if verb.lower() not in {"get", "post", "put", "patch", "delete", "head", "options", "trace"}:
                continue
            if not isinstance(op, dict) or not isinstance(op.get("operationId"), str):
                raise ValueError(f"missing source identity: {verb.upper()} {path}")
            sources.add((op["operationId"], verb.upper(), path))
    return sources


def observed_sources(bindings: dict[str, Any]) -> set[tuple[str, str, str]]:
    sources = set()
    for method, operation in bindings["operations"].items():
        source = operation.get("metadata", {}).get("source_operation", {})
        identity = (
            source.get("operation_id"),
            source.get("method"),
            source.get("path"),
        )
        if not all(isinstance(value, str) and value for value in identity):
            raise ValueError(f"missing source identity for emitted Rust method {method}")
        sources.add(identity)
    return sources


def assert_complete_sources(spec: dict[str, Any], bindings: dict[str, Any]) -> None:
    declared = expected_sources(spec)
    emitted = observed_sources(bindings)
    if declared != emitted:
        missing = sorted(declared - emitted)
        extra = sorted(emitted - declared)
        raise ValueError(f"source operation coverage mismatch: unemitted={missing}; unexpected={extra}")


def config_for_candidate(config: str) -> str:
    """Remove only the retired manifest-only producer option, not raw features."""
    parsed = tomllib.loads(config)
    if parsed.get("generator", {}).get("binding_manifest") is not True:
        raise ValueError("historical fixture does not explicitly request a producer manifest")
    lines = config.splitlines(keepends=True)
    chosen = [
        line for line in lines
        if re.fullmatch(r"\s*binding_manifest\s*=\s*true\s*(?:#.*)?(?:\r?\n)?", line)
    ]
    if len(chosen) != 1:
        raise ValueError("expected exactly one binding_manifest = true producer setting")
    candidate = "".join(line for line in lines if line not in chosen)
    if "binding_manifest" in tomllib.loads(candidate).get("generator", {}):
        raise ValueError("candidate config unexpectedly retains binding_manifest")
    return candidate


def raw_snapshot(raw: Path) -> dict[str, str]:
    return {
        file.relative_to(raw).as_posix(): digest(file.read_bytes())
        for file in sorted(raw.rglob("*"))
        if file.is_file() and file.name not in SKIPPED_RAW_FILES
    }


def invoke(*args: object, cwd: Path | None = None) -> str:
    command = [str(arg) for arg in args]
    completed = subprocess.run(command, cwd=cwd, text=True, capture_output=True, check=False)
    if completed.returncode:
        raise RuntimeError(
            f"command {command!r} failed (exit {completed.returncode}): "
            f"{completed.stderr.strip() or completed.stdout.strip()}"
        )
    return completed.stdout


def generate(binary: Path, spec_file: Path, config_text: str, directory: Path) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(spec_file, directory / "openapi.json")
    (directory / "config.toml").write_text(config_text)
    invoke(binary, "generate", "--config", "config.toml", cwd=directory)
    raw = directory / "raw"
    if not (raw / "client.rs").is_file() or not (raw / "types.rs").is_file():
        raise RuntimeError(f"raw_generation: missing generated client.rs/types.rs in {raw}")
    return raw


def read_bindings(adapter: Path, raw: Path, spec: Path | None) -> dict[str, Any]:
    command = (adapter, "--extract", raw, spec) if spec is not None else (adapter, raw)
    return normalized_bindings(json.loads(invoke(*command)))


def compare_scenario(
    fixture_root: Path,
    scenario: str,
    baseline: Path,
    candidate: Path,
    adapter: Path,
    work: Path,
) -> dict[str, Any]:
    fixture = fixture_root / scenario / "openapi.json"
    config = fixture_root / ("discriminator/fork.toml" if scenario == "discriminator" else "fork.toml")
    source = fixture.read_bytes()
    spec = json.loads(source)
    historical_config = config.read_text()
    modern_config = config_for_candidate(historical_config)
    row: dict[str, Any] = {
        "scenario": scenario,
        "source_openapi_sha256": digest(source),
        "effective_openapi_sha256": digest(source),
        "historical_config_sha256": digest(historical_config.encode()),
        "candidate_config_sha256": digest(modern_config.encode()),
        "candidate_config_delta": "retired binding_manifest option only",
        "baseline_stage": "not_run",
        "candidate_stage": "not_run",
        "canonical_parity": False,
        "raw_parity": False,
    }
    try:
        baseline_raw = generate(baseline, fixture, historical_config, work / scenario / "historical")
        row["baseline_stage"] = "raw_generated"
        if not (baseline_raw / "binding-manifest.json").is_file():
            raise RuntimeError("historical baseline did not emit its required manifest oracle")
        manifest = read_bindings(adapter, baseline_raw, None)
        extracted = read_bindings(adapter, baseline_raw, work / scenario / "historical" / "openapi.json")
        if manifest != extracted:
            raise RuntimeError("historical manifest oracle does not equal manifest-free extraction")
        assert_complete_sources(spec, extracted)
        row["baseline_stage"] = "manifest_oracle_and_manifest_free_proved"
        candidate_raw = generate(candidate, fixture, modern_config, work / scenario / "candidate")
        row["candidate_stage"] = "raw_generated"
        if (candidate_raw / "binding-manifest.json").exists():
            raise RuntimeError("current fork unexpectedly emitted a producer manifest")
        modern = read_bindings(adapter, candidate_raw, work / scenario / "candidate" / "openapi.json")
        assert_complete_sources(spec, modern)
        repeated = read_bindings(adapter, candidate_raw, work / scenario / "candidate" / "openapi.json")
        if modern != repeated:
            raise RuntimeError("nondeterministic manifest-free extraction")
        row["candidate_stage"] = "manifest_free_extracted_deterministically"
        row["canonical_parity"] = manifest == modern
        baseline_snapshot, candidate_snapshot = raw_snapshot(baseline_raw), raw_snapshot(candidate_raw)
        row["raw_parity"] = baseline_snapshot == candidate_snapshot
        row["changed_raw_files"] = sorted(
            name for name in baseline_snapshot.keys() | candidate_snapshot.keys()
            if baseline_snapshot.get(name) != candidate_snapshot.get(name)
        )
        row["baseline_operations"] = len(manifest["operations"])
        row["candidate_operations"] = len(modern["operations"])
        if not row["canonical_parity"]:
            raise RuntimeError("canonical Bindings v3 mismatch against historical manifest oracle")
        if not row["raw_parity"]:
            raise RuntimeError(
                "ordinary generated Rust/dependency drift requires review: "
                + ", ".join(row["changed_raw_files"])
            )
        row["status"] = "supported"
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        row["status"] = "failed"
        row["diagnostic"] = f"{type(error).__name__}: {error}"
    return row


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture-root", type=Path, required=True)
    parser.add_argument("--adapter", type=Path, required=True)
    parser.add_argument("--historical-fork", type=Path, required=True)
    parser.add_argument("--candidate-fork", type=Path, required=True)
    parser.add_argument("--historical-sha", required=True)
    parser.add_argument("--candidate-sha", required=True)
    parser.add_argument("--report-json", type=Path, required=True)
    parser.add_argument("--report-md", type=Path, required=True)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="post-manifest-fork-compat-") as temp:
        cases = [
            compare_scenario(
                args.fixture_root, scenario, args.historical_fork, args.candidate_fork,
                args.adapter, Path(temp),
            )
            for scenario in SCENARIOS
        ]
    passed = all(case["status"] == "supported" for case in cases)
    report = {
        "schema": "rust-sdk-generator.post-manifest-fork-compatibility",
        "schema_version": 1,
        "profile": "temporary_fork_raw_and_manifest_free",
        "historical_fork_sha": args.historical_sha,
        "candidate_fork_sha": args.candidate_sha,
        "upstream_default_switched": False,
        "historical_manifest_is_oracle_only": True,
        "scenarios": cases,
        "compatible": passed,
    }
    args.report_json.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    lines = [
        "# Post-manifest fork compatibility",
        "",
        f"- Historical manifest oracle: `{args.historical_sha}`",
        f"- Moving fork candidate (resolved immutable revision): `{args.candidate_sha}`",
        "- Production default: unchanged. #157 owns the final upstream/nightly migration.",
        "",
        "| Fixture | Historical oracle | Candidate ordinary Rust | Canonical parity | Raw parity | Result |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for case in cases:
        lines.append(
            f"| {case['scenario']} | {case['baseline_stage']} | {case['candidate_stage']} | "
            f"{case['canonical_parity']} | {case['raw_parity']} | {case['status']} |"
        )
        if case.get("diagnostic"):
            lines.append(f"\n**{case['scenario']}**: `{case['diagnostic']}`")
    lines.extend(["", f"**Result: {'compatible' if passed else 'incompatible'}.**", ""])
    args.report_md.write_text("\n".join(lines))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
