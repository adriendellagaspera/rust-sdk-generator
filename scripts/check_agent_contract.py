#!/usr/bin/env python3
"""Validate the repository's small, executable agent contract."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AGENT_FILE = ROOT / "AGENTS.md"
CLAUDE_FILE = ROOT / "CLAUDE.md"
WORKFLOWS = ROOT / ".github" / "workflows"
NORMATIVE = re.compile(r"\b(MUST|MUST NOT|NEVER|ALWAYS|DO NOT|REQUIRED)\b")
GATED_RULE = re.compile(r"^- \[([a-z][a-z0-9-]*)\] ")
JOB = re.compile(r"^  ([a-z][a-z0-9_-]*):\s*$")
USES = re.compile(r"^\s*-\s+uses:\s+([^\s#]+)")
PINNED_ACTION = re.compile(r"^[^@\s]+@[0-9a-f]{40}$")


def fail(message: str) -> None:
    raise SystemExit(f"agent-contract: {message}")


def instruction_files() -> set[Path]:
    files: set[Path] = set()
    for name in ("AGENTS.md", "CLAUDE.md"):
        files.update(path.relative_to(ROOT) for path in ROOT.rglob(name) if ".git" not in path.parts)
    return files


def workflow_jobs() -> set[str]:
    ci = (WORKFLOWS / "ci.yml").read_text()
    in_jobs = False
    jobs: set[str] = set()
    for line in ci.splitlines():
        if line == "jobs:":
            in_jobs = True
            continue
        if in_jobs and line and not line.startswith(" "):
            break
        if in_jobs and (match := JOB.match(line)):
            jobs.add(match.group(1))
    return jobs


def check_workflows() -> None:
    for workflow in sorted(WORKFLOWS.glob("*.yml")):
        text = workflow.read_text()
        if re.search(r"^\s*pull_request_target:\s*$", text, re.MULTILINE):
            fail(f"{workflow.relative_to(ROOT)} uses forbidden pull_request_target")
        for number, line in enumerate(text.splitlines(), start=1):
            match = USES.match(line)
            if not match:
                continue
            value = match.group(1)
            if value.startswith("./"):
                continue
            if not PINNED_ACTION.fullmatch(value):
                fail(f"{workflow.relative_to(ROOT)}:{number}: action is not pinned to a full SHA: {value}")


def main() -> None:
    expected = {Path("AGENTS.md"), Path("CLAUDE.md")}
    found = instruction_files()
    if found != expected:
        fail(f"agent instruction files must be exactly {sorted(map(str, expected))}; found {sorted(map(str, found))}")

    agents = AGENT_FILE.read_text()
    claude = CLAUDE_FILE.read_text()
    if claude != "@AGENTS.md\n":
        fail("CLAUDE.md must contain exactly '@AGENTS.md'")

    line_count = len(agents.splitlines()) + len(claude.splitlines())
    if line_count > 150:
        fail(f"AGENTS.md + CLAUDE.md exceed the 150-line budget ({line_count})")

    jobs = workflow_jobs()
    for number, line in enumerate(agents.splitlines(), start=1):
        if not NORMATIVE.search(line):
            continue
        match = GATED_RULE.match(line)
        if not match:
            fail(f"AGENTS.md:{number}: normative rule has no '[gate]' prefix")
        gate = match.group(1)
        if gate not in jobs:
            fail(f"AGENTS.md:{number}: references unknown CI gate '{gate}'")

    check_workflows()
    print(f"agent-contract: ok ({line_count}/150 instruction lines; gates={','.join(sorted(jobs))})")


if __name__ == "__main__":
    main()
