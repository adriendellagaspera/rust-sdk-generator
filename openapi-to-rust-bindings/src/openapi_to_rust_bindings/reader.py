"""Load normalized Bindings from openapi-to-rust output."""

from __future__ import annotations

import json
from pathlib import Path

from .model import Bindings
from .parser import ParseError, parse_bindings

SIDECAR_NAME = "rust-bindings.json"


def read_bindings(path: str | Path) -> Bindings:
    """Read normalized bindings, preferring a generator-owned sidecar when present."""
    path = Path(path)
    sidecar = path / SIDECAR_NAME
    if sidecar.is_file():
        try:
            value = json.loads(sidecar.read_text())
            return Bindings.from_dict(value)
        except (OSError, UnicodeError, json.JSONDecodeError, TypeError, ValueError) as error:
            raise ParseError(f"invalid {SIDECAR_NAME}") from error

    return parse_bindings(
        (path / "types.rs").read_bytes(),
        (path / "client.rs").read_bytes(),
    )
