#!/usr/bin/env python3
"""Fail-closed transitional watch for moving fork raw output without a manifest.

The pinned historical fork is only an oracle. This does not test upstream as
the production default; the production compatibility migration belongs to #157.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
from typing import Any


def command(*args: object, cwd: Path | None = None) -> str:
    argv = [str(arg) for arg in args]
    result = subprocess.run(argv, cwd=cwd, capture_output=True, text=True, check=False)
    if result.returncode:
        raise RuntimeError(
            f"{argv!r} exited {result.returncode}: "
            f"{result.stderr.strip() or result.stdout.strip()}"
        )
    return result.stdout


def snapshot(directory: Path) -> dict[str, bytes]:
    return {
        path.relative_to(directory).as_posix(): path.read_bytes()
        for path in sorted(directory.rglob("*"))
        if path.is_file() and path.name != "binding-manifest.json"
    }


def canonical(bindings: dict[str, Any]) -> dict[str, Any]:
    if bindings.get("schema_version") != 3 or not isinstance(bindings.get("operations"), dict):
        raise ValueError("expected canonical Bindings v3 operations object")
    value = json.loads(json.dumps(bindings))
    for operation in value["operations"].values():
        if not isinstance(operation, dict):
            raise ValueError("invalid canonical operation")
        return_type = operation.get("return_type")
        if isinstance(return_type, str):
            operation["return_type"] = normalize_type(return_type)
        abi = operation.get("metadata", {}).get("stream_abi")
        if isinstance(abi, dict):
            for target in ("native_type", "wasm_type"):
                if isinstance(abi.get(target), str):
                    abi[target] = normalize_type(abi[target])
    return value


def normalize_type(value: str) -> str:
    return re.sub(r",(?=>)", "", re.sub(r"\s+", "", value))


def assert_source_coverage(spec: dict[str, Any], bindings: dict[str, Any]) -> None:
    declared: set[tuple[str, str, str]] = set()
    for path, item in spec["paths"].items():
        for method, operation in item.items():
            if method.lower() not in {"get", "post", "put", "patch", "delete", "head", "options", "trace"}:
                continue
            declared.add((operation["operationId"], method.upper(), path))
    observed: set[tuple[str, str, str]] = set()
    for name, operation in bindings["operations"].items():
        source = operation.get("metadata", {}).get("source_operation")
        if not isinstance(source, dict):
            raise ValueError(f"operation {name} has no canonical source identity")
        identity = (source.get("operation_id"), source.get("method"), source.get("path"))
        if not all(isinstance(part, str) and part for part in identity):
            raise ValueError(f"operation {name} has incomplete canonical source identity")
        observed.add(identity)
    if declared != observed:
        raise ValueError(
            f"source coverage mismatch: unemitted={sorted(declared-observed)}; "
            f"unexpected={sorted(observed-declared)}"
        )


def prepare(fixture: Path, destination: Path, manifest: bool) -> tuple[Path, Path]:
    shutil.copytree(fixture, destination)
    config = destination / "compat.toml"
    text = config.read_text()
    matches = re.findall(r"^binding_manifest\s*=\s*true\s*$", text, re.MULTILINE)
    if len(matches) != 1:
        raise ValueError("the historical fixture must explicitly request exactly one manifest")
    if not manifest:
        text = re.sub(r"^binding_manifest\s*=\s*true\s*\n", "", text, count=1, flags=re.MULTILINE)
        if "binding_manifest" in text:
            raise ValueError("candidate config still contains a manifest producer setting")
        config.write_text(text)
    return config, destination / "openapi.json"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--historical-generator", required=True, type=Path)
    parser.add_argument("--candidate-generator", required=True, type=Path)
    parser.add_argument("--historical-sha", required=True)
    parser.add_argument("--candidate-sha", required=True)
    parser.add_argument("--adapter", required=True, type=Path)
    parser.add_argument("--fixture", required=True, type=Path)
    parser.add_argument("--report-json", required=True, type=Path)
    parser.add_argument("--report-md", required=True, type=Path)
    args = parser.parse_args()

    report: dict[str, Any] = {
        "schema_version": 1,
        "profile": "temporary_moving_fork_manifest_free_raw_watch",
        "historical_oracle_sha": args.historical_sha,
        "moving_fork_sha": args.candidate_sha,
        "production_default_changed": False,
        "stage": "configuration",
        "compatible": False,
    }
    try:
        if not all(re.fullmatch(r"[0-9a-f]{40}", sha) for sha in
                   (args.historical_sha, args.candidate_sha)):
            raise ValueError("backend revisions must be resolved immutable 40-character SHAs")
        source = (args.fixture / "openapi.json").read_bytes()
        report["effective_openapi_sha256"] = hashlib.sha256(source).hexdigest()
        spec = json.loads(source)
        with tempfile.TemporaryDirectory(prefix="moving-fork-compat-") as temporary:
            root = Path(temporary)
            before_config, before_spec = prepare(args.fixture, root / "historical", True)
            after_config, after_spec = prepare(args.fixture, root / "candidate", False)
            report["historical_config_sha256"] = hashlib.sha256(before_config.read_bytes()).hexdigest()
            report["candidate_config_sha256"] = hashlib.sha256(after_config.read_bytes()).hexdigest()
            report["stage"] = "historical_raw_generation"
            command(args.historical_generator, "generate", "--config", before_config, cwd=before_config.parent)
            before_raw = before_config.parent / "raw"
            if not (before_raw / "binding-manifest.json").is_file():
                raise RuntimeError("historical oracle failed to produce its manifest")
            report["stage"] = "historical_oracle_extraction"
            historical = canonical(json.loads(command(args.adapter, before_raw)))
            historical_extracted = canonical(json.loads(
                command(args.adapter, "--extract", before_raw, before_spec)
            ))
            if historical != historical_extracted:
                raise RuntimeError("historical manifest and manifest-free extraction differ")
            assert_source_coverage(spec, historical)
            report["stage"] = "candidate_raw_generation"
            command(args.candidate_generator, "generate", "--config", after_config, cwd=after_config.parent)
            after_raw = after_config.parent / "raw"
            if (after_raw / "binding-manifest.json").exists():
                raise RuntimeError("moving fork unexpectedly emitted a producer manifest")
            for filename in ("client.rs", "types.rs", "mod.rs", "REQUIRED_DEPS.toml"):
                if not (before_raw / filename).is_file() or not (after_raw / filename).is_file():
                    raise RuntimeError(f"missing ordinary generated Rust artifact: {filename}")
            before_files, after_files = snapshot(before_raw), snapshot(after_raw)
            changed = sorted(
                name for name in before_files.keys() | after_files.keys()
                if before_files.get(name) != after_files.get(name)
            )
            report["changed_raw_files"] = changed
            report["stage"] = "candidate_manifest_free_extraction"
            candidate = canonical(json.loads(
                command(args.adapter, "--extract", after_raw, after_spec)
            ))
            assert_source_coverage(spec, candidate)
            repeated = canonical(json.loads(
                command(args.adapter, "--extract", after_raw, after_spec)
            ))
            if candidate != repeated:
                raise RuntimeError("moving fork manifest-free extraction is nondeterministic")
            report["historical_operation_count"] = len(historical["operations"])
            report["candidate_operation_count"] = len(candidate["operations"])
            report["stage"] = "canonical_and_raw_parity"
            if historical != candidate:
                raise RuntimeError("moving fork changed canonical Bindings v3 relative to historical oracle")
            if changed:
                raise RuntimeError(f"moving fork changed emitted Rust; review files {changed}")
        report["compatible"] = True
        report["stage"] = "passed"
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, subprocess.CalledProcessError) as error:
        report["diagnostic"] = f"{type(error).__name__}: {error}"

    args.report_json.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    args.report_md.write_text(
        "# Moving fork compatibility (temporary)\n\n"
        f"- Historical oracle SHA: `{args.historical_sha}`\n"
        f"- Current fork resolved SHA: `{args.candidate_sha}`\n"
        f"- Effective OpenAPI SHA-256: `{report.get('effective_openapi_sha256', 'unavailable')}`\n"
        f"- Last stage: `{report['stage']}`\n"
        f"- Changed raw files: `{report.get('changed_raw_files', 'not compared')}`\n"
        f"- Result: **{'compatible' if report['compatible'] else 'incompatible'}**\n"
        f"- Diagnostic: `{report.get('diagnostic', 'none')}`\n\n"
        "This checks the moving fork against a historical oracle, not the unmodified "
        "upstream production boundary. The complete upstream nightly migration is #157.\n"
    )
    return 0 if report["compatible"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
