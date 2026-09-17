#!/usr/bin/env python3
"""Check pull-request titles against the repository convention."""

from __future__ import annotations

import re
import sys

PATTERN = re.compile(
    r"^(?:feat|fix|refactor|perf|docs|test|ci|build|chore)"
    r"(?:\([a-z0-9][a-z0-9._/-]*\))?!?: .+"
)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: check_pr_title.py '<title>'")
    title = sys.argv[1].strip()
    if title.startswith('Revert "'):
        return
    if not PATTERN.fullmatch(title):
        raise SystemExit(
            "PR title must use <type>(<optional-scope>): <summary>; "
            "allowed types: feat, fix, refactor, perf, docs, test, ci, build, chore"
        )


if __name__ == "__main__":
    main()
