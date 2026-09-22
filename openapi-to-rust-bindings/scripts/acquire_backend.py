#!/usr/bin/env python3
"""Acquire an immutable openapi-to-rust source checkout from a configured repository."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess
import sys


SHA = re.compile(r"[0-9a-f]{40}\Z")
REPOSITORY = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+\Z")


def run(*command: object) -> str:
    result = subprocess.run(
        [str(part) for part in command],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode:
        raise RuntimeError(
            f"{' '.join(str(part) for part in command)} failed "
            f"({result.returncode}): {result.stderr or result.stdout}"
        )
    return result.stdout.strip()


def acquire(repository: str, ref: str, destination: Path, expected_sha: str | None = None) -> str:
    """Resolve an arbitrary candidate ref, or verify an expected immutable pin."""
    if not REPOSITORY.fullmatch(repository):
        raise ValueError("repository must be a GitHub owner/name")
    if not ref or ref.startswith("-"):
        raise ValueError("candidate ref must not be empty or start with '-'")
    if expected_sha is not None and not SHA.fullmatch(expected_sha):
        raise ValueError("expected SHA must contain 40 lowercase hex characters")
    if destination.exists() and any(destination.iterdir()):
        raise ValueError(f"backend destination is not empty: {destination}")
    destination.mkdir(parents=True, exist_ok=True)
    run("git", "init", destination)
    run("git", "-C", destination, "remote", "add", "origin", f"https://github.com/{repository}.git")
    run("git", "-C", destination, "fetch", "--depth=1", "origin", ref)
    resolved = run("git", "-C", destination, "rev-parse", "FETCH_HEAD")
    if not SHA.fullmatch(resolved):
        raise RuntimeError(f"backend did not resolve to an immutable commit: {resolved}")
    if expected_sha is not None and resolved != expected_sha:
        raise RuntimeError(f"backend pin mismatch: expected {expected_sha}, resolved {resolved}")
    run("git", "-C", destination, "checkout", "--detach", resolved)
    actual = run("git", "-C", destination, "rev-parse", "HEAD")
    if actual != resolved:
        raise RuntimeError(f"checkout mismatch: resolved {resolved}, actual {actual}")
    return resolved


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--ref", required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--expected-sha")
    args = parser.parse_args()
    try:
        print(acquire(args.repository, args.ref, args.destination, args.expected_sha))
    except (OSError, RuntimeError, ValueError) as error:
        print(f"[backend acquisition] {error}", file=sys.stderr)
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
