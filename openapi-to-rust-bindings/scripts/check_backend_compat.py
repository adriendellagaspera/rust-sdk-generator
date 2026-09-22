#!/usr/bin/env python3
"""Compare immutable openapi-to-rust revisions at the canonical Bindings boundary."""

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

from compat_fixtures import discover_fixtures


def run(*args: object, cwd: Path | None = None) -> None:
    subprocess.run([str(arg) for arg in args], cwd=cwd, check=True)


def output(*args: object, cwd: Path | None = None) -> str:
    return subprocess.check_output(
        [str(arg) for arg in args], cwd=cwd, text=True
    ).strip()


def bindings_value(adapter: Path, generated: Path) -> dict[str, Any]:
    value = json.loads(output(adapter, generated))
    if not isinstance(value, dict):
        raise RuntimeError("bindings adapter did not emit a JSON object")
    if value.get("schema_version") != 3:
        raise RuntimeError(
            f"manifest compatibility requires Bindings v3, got {value.get('schema_version')!r}"
        )
    return value


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


def changed_line_counts(before: bytes, after: bytes) -> tuple[int, int]:
    added = removed = 0
    for line in difflib.ndiff(
        before.decode(errors="replace").splitlines(),
        after.decode(errors="replace").splitlines(),
    ):
        if line.startswith("+ "):
            added += 1
        elif line.startswith("- "):
            removed += 1
    return added, removed


def generator_version(binary: Path) -> str:
    actual = output(binary, "--version")
    match = re.fullmatch(r"openapi-to-rust\s+(.+)", actual)
    if not match:
        raise RuntimeError(f"unexpected generator version output: {actual!r}")
    return match.group(1)


def prepared_spec(source: Path, destination: Path) -> None:
    document = json.loads(source.read_text())
    document.setdefault(
        "info", {"title": "bindings compatibility fixture", "version": "1"}
    )
    destination.write_text(json.dumps(document, indent=2) + "\n")


def generate_fixture(binary: Path, fixture: Path, destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    work = destination.parent
    spec = work / "openapi.json"
    prepared_spec(fixture / "openapi.json", spec)
    config = work / "openapi-to-rust.toml"
    fixture_config = fixture / "compat.toml"
    if fixture_config.is_file():
        for asset in fixture.glob("compat.*"):
            if asset.name != fixture_config.name:
                shutil.copy2(asset, work / asset.name)
        config.write_text(fixture_config.read_text())
    else:
        config.write_text(
            "\n".join(
                [
                    "[generator]",
                    f'spec_path = "{spec.as_posix()}"',
                    f'output_dir = "{destination.as_posix()}"',
                    'module_name = "fixture"',
                    "binding_manifest = true",
                    "",
                    "[features]",
                    "enable_async_client = true",
                    "",
                    "[http_client]",
                    'base_url = "https://example.invalid"',
                    "",
                    "[http_client.retry]",
                    "max_retries = 0",
                    "",
                ]
            )
        )
    run(binary, "generate", "--config", config)
    manifest = destination / "binding-manifest.json"
    if not manifest.is_file():
        raise RuntimeError(
            f"{binary} did not emit binding-manifest.json for fixture {fixture.name}"
        )


def changed_sections(before: dict[str, Any], after: dict[str, Any]) -> list[str]:
    return sorted(
        key for key in before.keys() | after.keys() if before.get(key) != after.get(key)
    )


def canonical_json(value: Any) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def binding_identities(
    bindings: dict[str, Any],
) -> tuple[set[str], set[str]]:
    operations = bindings.get("operations")
    if not isinstance(operations, dict):
        raise RuntimeError("Bindings v3 operations must be an object")

    sources: set[str] = set()
    representations: set[str] = set()
    for method_name, operation in sorted(operations.items()):
        if not isinstance(operation, dict):
            raise RuntimeError(f"operation {method_name!r} must be an object")
        metadata = operation.get("metadata")
        if not isinstance(metadata, dict):
            raise RuntimeError(f"operation {method_name!r} is missing v3 metadata")
        source = metadata.get("source_operation")
        representation = metadata.get("representation")
        kind = metadata.get("kind")
        if not isinstance(source, dict) or not isinstance(representation, dict):
            raise RuntimeError(
                f"operation {method_name!r} has incomplete canonical identity"
            )
        source_identity = canonical_json(
            {
                "operation_id": source.get("operation_id"),
                "method": source.get("method"),
                "path": source.get("path"),
            }
        )
        sources.add(source_identity)
        representations.add(
            canonical_json(
                {
                    "kind": kind,
                    "source_operation": json.loads(source_identity),
                    "representation": representation,
                }
            )
        )
    return sources, representations


def identity_diff(
    baseline: set[str], candidate: set[str]
) -> tuple[list[str], list[str]]:
    return sorted(candidate - baseline), sorted(baseline - candidate)


def display_identity(value: str) -> str:
    parsed = json.loads(value)
    source = parsed.get("source_operation", parsed)
    operation_id = source.get("operation_id")
    method = source.get("method")
    path = source.get("path")
    if "representation" not in parsed:
        return f"{operation_id} [{method} {path}]"
    representation = parsed["representation"]
    kind = representation.get("kind")
    media = representation.get("media_type")
    suffix = f" {media}" if media else ""
    return (
        f"{operation_id} [{method} {path}] "
        f"{parsed.get('kind')} -> {kind}{suffix}"
    )


def compatibility_config(package_root: Path) -> dict[str, Any]:
    value = json.loads((package_root / "COMPATIBILITY.json").read_text())
    backend = value.get("backend")
    baseline = backend.get("baseline") if isinstance(backend, dict) else None
    if (
        value.get("schema_version") != 2
        or value.get("bindings_schema_version") != 3
        or not isinstance(backend, dict)
        or not isinstance(baseline, dict)
        or not isinstance(backend.get("repository"), str)
        or not isinstance(baseline.get("version"), str)
        or not isinstance(baseline.get("commit"), str)
    ):
        raise RuntimeError("COMPATIBILITY.json is not a manifest-era v2 tracker config")
    return value


def update_compatibility(
    package_root: Path,
    compatibility: dict[str, Any],
    candidate_version: str,
    candidate_commit: str,
) -> None:
    updated = json.loads(json.dumps(compatibility))
    updated["backend"]["baseline"] = {
        "version": candidate_version,
        "commit": candidate_commit,
    }
    (package_root / "COMPATIBILITY.json").write_text(
        json.dumps(updated, indent=2) + "\n"
    )


def update_data(
    compatibility: dict[str, Any],
    candidate_version: str,
    candidate_commit: str,
    fixtures: list[str],
) -> dict[str, Any]:
    return {
        "backend": {
            "name": compatibility["backend"]["name"],
            "repository": compatibility["backend"]["repository"],
            "previous_baseline": compatibility["backend"]["baseline"],
            "proposed_baseline": {
                "version": candidate_version,
                "commit": candidate_commit,
            },
        },
        "bindings_schema_version": compatibility["bindings_schema_version"],
        "fixtures": fixtures,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package-root", type=Path, default=Path("."))
    parser.add_argument("--bindings-adapter", type=Path, required=True)
    parser.add_argument("--baseline-generator", type=Path, required=True)
    parser.add_argument("--candidate-generator", type=Path, required=True)
    parser.add_argument("--candidate-commit", required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--update-data", type=Path)
    parser.add_argument("--report-json", type=Path, help="Stage-aware legacy oracle report")
    parser.add_argument("--update-compatibility", action="store_true")
    args = parser.parse_args()

    package_root = args.package_root.resolve()
    compatibility = compatibility_config(package_root)
    baseline = compatibility["backend"]["baseline"]
    baseline_commit = baseline["commit"]
    baseline_version = generator_version(args.baseline_generator)
    candidate_version = generator_version(args.candidate_generator)
    if baseline_version != baseline["version"]:
        raise RuntimeError(
            f"baseline generator version mismatch: "
            f"{baseline_version} != {baseline['version']}"
        )
    if not re.fullmatch(r"[0-9a-f]{40}", baseline_commit):
        raise RuntimeError("baseline commit must be an immutable 40-character SHA")
    if not re.fullmatch(r"[0-9a-f]{40}", args.candidate_commit):
        raise RuntimeError("candidate commit must be resolved to an immutable SHA")

    fixture_root = package_root / "tests" / "fixtures"
    fixture_names = [
        path.relative_to(fixture_root).as_posix()
        for path in discover_fixtures(fixture_root, "legacy_manifest_oracle")
    ]
    if not fixture_names:
        raise RuntimeError("no bindings compatibility fixtures found")

    rows: list[dict[str, Any]] = []
    incompatible: list[str] = []
    with tempfile.TemporaryDirectory(prefix="openapi-to-rust-bindings-") as temporary:
        work = Path(temporary)
        for name in fixture_names:
            fixture = fixture_root / name
            baseline_raw = work / name / "baseline" / "raw"
            candidate_raw = work / name / "candidate" / "raw"

            baseline_raw_error = candidate_raw_error = None
            try:
                generate_fixture(args.baseline_generator, fixture, baseline_raw)
            except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
                baseline_raw_error = f"{type(error).__name__}: {error}"
            try:
                generate_fixture(args.candidate_generator, fixture, candidate_raw)
            except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
                candidate_raw_error = f"{type(error).__name__}: {error}"

            baseline_snapshot = snapshot(baseline_raw) if baseline_raw_error is None else {}
            candidate_snapshot = snapshot(candidate_raw) if candidate_raw_error is None else {}
            raw_changed = (
                changed_files(baseline_snapshot, candidate_snapshot)
                if baseline_raw_error is None and candidate_raw_error is None
                else []
            )
            raw_added = raw_removed = 0
            for filename in raw_changed:
                added, removed = changed_line_counts(
                    baseline_snapshot.get(filename, b""),
                    candidate_snapshot.get(filename, b""),
                )
                raw_added += added
                raw_removed += removed

            baseline_error = candidate_error = None
            baseline_value = candidate_value = None
            baseline_sources: set[str] = set()
            baseline_representations: set[str] = set()
            candidate_sources: set[str] = set()
            candidate_representations: set[str] = set()
            try:
                if baseline_raw_error:
                    raise RuntimeError(f"raw_generation_failure: {baseline_raw_error}")
                baseline_value = bindings_value(args.bindings_adapter, baseline_raw)
                baseline_sources, baseline_representations = binding_identities(
                    baseline_value
                )
            except Exception as error:
                baseline_error = f"{type(error).__name__}: {error}"
            try:
                if candidate_raw_error:
                    raise RuntimeError(f"raw_generation_failure: {candidate_raw_error}")
                candidate_value = bindings_value(args.bindings_adapter, candidate_raw)
                candidate_sources, candidate_representations = binding_identities(
                    candidate_value
                )
            except Exception as error:
                candidate_error = f"{type(error).__name__}: {error}"

            sections = (
                changed_sections(baseline_value, candidate_value)
                if baseline_value is not None and candidate_value is not None
                else []
            )
            source_added, source_removed = (
                identity_diff(baseline_sources, candidate_sources)
                if baseline_value is not None and candidate_value is not None else ([], [])
            )
            representation_added, representation_removed = (
                identity_diff(baseline_representations, candidate_representations)
                if baseline_value is not None and candidate_value is not None else ([], [])
            )
            compatible = (
                candidate_error is None
                and baseline_error is None
                and not sections
                and not source_added
                and not source_removed
                and not representation_added
                and not representation_removed
            )
            if not compatible:
                incompatible.append(name)

            def provenance(raw: Path, generation_failed: bool) -> dict[str, Any]:
                work = raw.parent
                config = work / "openapi-to-rust.toml"
                has_overlay = (
                    config.is_file() and 'overlays = [' in config.read_text()
                )
                overlay = work / "openapi.overlaid.json"
                effective = (
                    None if generation_failed
                    else overlay if overlay.is_file()
                    else None if has_overlay
                    else work / "openapi.json"
                )
                return {
                    "source_openapi_sha256": hashlib.sha256(
                        (fixture / "openapi.json").read_bytes()
                    ).hexdigest(),
                    "effective_openapi_sha256": (
                        hashlib.sha256(effective.read_bytes()).hexdigest()
                        if effective is not None and effective.is_file() else None
                    ),
                    "effective_openapi_file": effective.name if effective is not None and effective.is_file() else None,
                    "effective_openapi_status": (
                        "unavailable_generation_failed" if generation_failed
                        else "unverified_missing_overlay_output" if effective is None
                        else "recorded" if effective.is_file()
                        else "unavailable"
                    ),
                    "generation_config_sha256": (
                        hashlib.sha256(config.read_bytes()).hexdigest()
                        if config.is_file() else None
                    ),
                    "generation_config": config.read_text() if config.is_file() else None,
                }

            rows.append(
                {
                    "fixture": name,
                    "provenance": {
                        "baseline": provenance(baseline_raw, baseline_raw_error is not None),
                        "candidate": provenance(candidate_raw, candidate_raw_error is not None),
                    },
                    "raw_changed": raw_changed,
                    "raw_added_lines": raw_added,
                    "raw_removed_lines": raw_removed,
                    "baseline_error": baseline_error,
                    "candidate_error": candidate_error,
                    "stages": {
                        "baseline_raw_generation": "failed" if baseline_raw_error else "passed",
                        "candidate_raw_generation": "failed" if candidate_raw_error else "passed",
                        "baseline_adapter_evidence": "not_run" if baseline_raw_error else ("failed" if baseline_error else "passed"),
                        "candidate_adapter_evidence": "not_run" if candidate_raw_error else ("failed" if candidate_error else "passed"),
                        "root_sdk_derivation": "not_run_legacy_oracle",
                    },
                    "failure_stage": (
                        "raw_generation" if baseline_raw_error or candidate_raw_error
                        else "adapter_evidence" if baseline_error or candidate_error
                        else None
                    ),
                    "bindings_changed": sections,
                    "source_added": source_added,
                    "source_removed": source_removed,
                    "representation_added": representation_added,
                    "representation_removed": representation_removed,
                    "raw_diff": {
                        filename: "".join(difflib.unified_diff(
                            baseline_snapshot.get(filename, b"").decode(errors="replace").splitlines(keepends=True),
                            candidate_snapshot.get(filename, b"").decode(errors="replace").splitlines(keepends=True),
                            fromfile=f"baseline/{filename}",
                            tofile=f"candidate/{filename}",
                        ))
                        for filename in raw_changed
                        if filename.endswith((".rs", ".toml"))
                    },
                }
            )

    lines = [
        "# openapi-to-rust canonical Bindings compatibility",
        "",
        f"- Backend repository: `{compatibility['backend']['repository']}`",
        f"- Baseline: `{baseline_version}` / `{baseline_commit}`",
        f"- Candidate: `{candidate_version}` / `{args.candidate_commit}`",
        f"- Bindings schema: `{compatibility['bindings_schema_version']}`",
        "- Profile: **historical manifest oracle**, not production manifest-free compatibility",
        "- Authority: generator-owned `binding-manifest.json` normalized to canonical Bindings",
        "",
        "| Fixture | Raw generator diff | Normalization | Bindings diff | Source identity | Representation identity |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for row in rows:
        raw = (
            "none"
            if not row["raw_changed"]
            else f"{len(row['raw_changed'])} files (+{row['raw_added_lines']}/-{row['raw_removed_lines']})"
        )
        normalization = (
            f"baseline error: `{row['baseline_error']}`"
            if row["baseline_error"]
            else f"candidate error: `{row['candidate_error']}`"
            if row["candidate_error"]
            else "ok"
        )
        bindings_diff = (
            "none"
            if not row["bindings_changed"]
            else ", ".join(f"`{section}`" for section in row["bindings_changed"])
        )
        source_diff = (
            "same"
            if not row["source_added"] and not row["source_removed"]
            else f"+{len(row['source_added'])}/-{len(row['source_removed'])}"
        )
        representation_diff = (
            "same"
            if not row["representation_added"] and not row["representation_removed"]
            else (
                f"+{len(row['representation_added'])}/"
                f"-{len(row['representation_removed'])}"
            )
        )
        lines.append(
            f"| {row['fixture']} | {raw} | {normalization} | {bindings_diff} | "
            f"{source_diff} | {representation_diff} |"
        )

    for row in rows:
        details: list[str] = []
        for label, values in [
            ("Source identities added", row["source_added"]),
            ("Source identities removed", row["source_removed"]),
            ("Representation identities added", row["representation_added"]),
            ("Representation identities removed", row["representation_removed"]),
        ]:
            if values:
                details.extend(
                    [f"- {label}:"] + [f"  - `{display_identity(value)}`" for value in values]
                )
        if details:
            lines.extend(["", f"## {row['fixture']} identity drift", "", *details])
        if row["raw_diff"]:
            lines.extend(["", f"## {row['fixture']} generated-output differences", ""])
            for filename, diff in sorted(row["raw_diff"].items()):
                lines.extend([f"### `{filename}`", "", "```diff",
                              *diff.splitlines()[:200], "```", ""])
                if len(diff.splitlines()) > 200:
                    lines.append("Diff preview truncated at 200 lines; inspect the raw artifacts for the full diff.")

    lines.extend(
        [
            "",
            (
                "**Result: compatible.** Canonical Bindings, source-operation identities, "
                "and representation identities are identical."
                if not incompatible
                else "**Result: incompatible.** Canonical compatibility changed or "
                "candidate normalization failed for: "
                + ", ".join(f"`{name}`" for name in incompatible)
            ),
            "",
            "Generated Rust diffs are diagnostic only; compatibility is defined at "
            "the canonical Bindings boundary.",
        ]
    )
    args.report.write_text("\n".join(lines) + "\n")
    if args.report_json is not None:
        report = {
            "schema_version": 1,
            "profile": "legacy_manifest_oracle",
            "backend": {
                "repository": compatibility["backend"]["repository"],
                "baseline": {"version": baseline_version, "commit": baseline_commit},
                "candidate": {"version": candidate_version, "commit": args.candidate_commit},
            },
            "bindings_schema_version": compatibility["bindings_schema_version"],
            "source_api_drift": [
                {"fixture": row["fixture"], "added": row["source_added"], "removed": row["source_removed"]}
                for row in rows
            ],
            "fixture_results": rows,
            "rejected_operations": {"status": "not_run_legacy_oracle"},
            "capability_differences": {
                "status": "not_evaluated_legacy_oracle",
                "producer_audit": "openapi-to-rust-bindings/FORK_CAPABILITIES.json",
            },
            "effective_openapi_provenance": {
                row["fixture"]: row["provenance"] for row in rows
            },
            "compatible": not incompatible,
            "rerun": {
                "workflow": ".github/workflows/openapi-to-rust-compat.yml",
                "candidate_ref": args.candidate_commit,
                "required_cli_flags": [
                    "--package-root", str(package_root),
                    "--bindings-adapter", str(args.bindings_adapter),
                    "--baseline-generator", str(args.baseline_generator),
                    "--candidate-generator", str(args.candidate_generator),
                    "--candidate-commit", args.candidate_commit,
                    "--report", str(args.report),
                ],
            },
        }
        args.report_json.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")

    if incompatible:
        raise SystemExit(1)

    data = update_data(
        compatibility, candidate_version, args.candidate_commit, fixture_names
    )
    if args.update_data is not None:
        args.update_data.write_text(json.dumps(data, indent=2) + "\n")
    if args.update_compatibility:
        update_compatibility(
            package_root, compatibility, candidate_version, args.candidate_commit
        )


if __name__ == "__main__":
    main()
