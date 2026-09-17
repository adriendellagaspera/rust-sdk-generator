#!/usr/bin/env python3
"""Check whether a newer openapi-to-rust revision preserves normalized Bindings."""

from __future__ import annotations

import argparse
import difflib
import json
from pathlib import Path
import re
import subprocess
import tempfile
from typing import Any

from openapi_to_rust_bindings import read_bindings


def run(*args: object, cwd: Path | None = None) -> None:
    subprocess.run([str(arg) for arg in args], cwd=cwd, check=True)


def output(*args: object, cwd: Path | None = None) -> str:
    return subprocess.check_output(
        [str(arg) for arg in args], cwd=cwd, text=True
    ).strip()


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
    spec = destination.parent / "openapi.json"
    prepared_spec(fixture / "openapi.json", spec)
    config = destination.parent / "openapi-to-rust.toml"
    config.write_text(
        "\n".join(
            [
                "[generator]",
                f'spec_path = "{spec.as_posix()}"',
                f'output_dir = "{destination.as_posix()}"',
                'module_name = "fixture"',
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


def changed_sections(before: dict[str, Any], after: dict[str, Any]) -> list[str]:
    return sorted(
        key for key in before.keys() | after.keys() if before.get(key) != after.get(key)
    )


def update_compatibility(
    package_root: Path, candidate_version: str, candidate_commit: str
) -> None:
    compatibility_path = package_root / "COMPATIBILITY.json"
    compatibility = json.loads(compatibility_path.read_text())
    compatibility["backend"]["version"] = candidate_version
    compatibility["backend"]["commit"] = candidate_commit
    compatibility_path.write_text(json.dumps(compatibility, indent=2) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package-root", type=Path, default=Path("."))
    parser.add_argument("--baseline-generator", type=Path, required=True)
    parser.add_argument("--candidate-generator", type=Path, required=True)
    parser.add_argument("--candidate-commit", required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--update-compatibility", action="store_true")
    args = parser.parse_args()

    package_root = args.package_root.resolve()
    compatibility = json.loads((package_root / "COMPATIBILITY.json").read_text())
    baseline_commit = compatibility["backend"]["commit"]
    baseline_version = generator_version(args.baseline_generator)
    candidate_version = generator_version(args.candidate_generator)
    expected_baseline = compatibility["backend"]["version"]
    if baseline_version != expected_baseline:
        raise RuntimeError(
            f"baseline generator version mismatch: {baseline_version} != {expected_baseline}"
        )

    fixture_root = package_root / "tests" / "fixtures"
    fixture_names = sorted(
        path.name
        for path in fixture_root.iterdir()
        if path.is_dir() and (path / "openapi.json").exists()
    )
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

            generate_fixture(args.baseline_generator, fixture, baseline_raw)
            generate_fixture(args.candidate_generator, fixture, candidate_raw)

            baseline_snapshot = snapshot(baseline_raw)
            candidate_snapshot = snapshot(candidate_raw)
            raw_changed = changed_files(baseline_snapshot, candidate_snapshot)
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
            try:
                baseline_value = read_bindings(baseline_raw).to_dict()
            except Exception as error:
                baseline_error = f"{type(error).__name__}: {error}"
            try:
                candidate_value = read_bindings(candidate_raw).to_dict()
            except Exception as error:
                candidate_error = f"{type(error).__name__}: {error}"

            if baseline_error is not None:
                raise RuntimeError(
                    f"baseline fixture {name} no longer parses: {baseline_error}"
                )

            sections = (
                changed_sections(baseline_value, candidate_value)
                if candidate_value is not None
                else []
            )
            compatible = candidate_error is None and not sections
            if not compatible:
                incompatible.append(name)

            rows.append(
                {
                    "fixture": name,
                    "raw_changed": raw_changed,
                    "raw_added_lines": raw_added,
                    "raw_removed_lines": raw_removed,
                    "candidate_error": candidate_error,
                    "bindings_changed": sections,
                }
            )

    lines = [
        "# openapi-to-rust Bindings compatibility",
        "",
        f"- Baseline: `{baseline_version}` / `{baseline_commit}`",
        f"- Candidate: `{candidate_version}` / `{args.candidate_commit}`",
        f"- Bindings package: `{compatibility['package_version']}`",
        f"- Bindings schema: `{compatibility['bindings_schema_version']}`",
        "",
        "| Fixture | Raw generator diff | Normalization | Bindings diff |",
        "| --- | --- | --- | --- |",
    ]
    for row in rows:
        raw = (
            "none"
            if not row["raw_changed"]
            else f"{len(row['raw_changed'])} files (+{row['raw_added_lines']}/-{row['raw_removed_lines']})"
        )
        normalization = (
            f"error: `{row['candidate_error']}`"
            if row["candidate_error"]
            else "ok"
        )
        bindings_diff = (
            "none"
            if not row["bindings_changed"]
            else ", ".join(f"`{section}`" for section in row["bindings_changed"])
        )
        lines.append(
            f"| {row['fixture']} | {raw} | {normalization} | {bindings_diff} |"
        )

    lines.extend(
        [
            "",
            (
                "**Result: compatible.** Canonical normalized Bindings are identical."
                if not incompatible
                else "**Result: incompatible.** Bindings changed or candidate normalization failed for: "
                + ", ".join(f"`{name}`" for name in incompatible)
            ),
            "",
            "Raw Rust diffs are diagnostic only; compatibility is defined at the Bindings boundary.",
        ]
    )
    args.report.write_text("\n".join(lines) + "\n")

    if incompatible:
        raise SystemExit(1)

    if args.update_compatibility:
        update_compatibility(package_root, candidate_version, args.candidate_commit)


if __name__ == "__main__":
    main()
