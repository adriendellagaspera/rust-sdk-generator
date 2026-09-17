"""Versioned normalized Bindings contract produced by this adapter."""

from __future__ import annotations

from copy import deepcopy
from dataclasses import dataclass
import importlib.resources
import json
from types import MappingProxyType
from typing import Any, Mapping

from jsonschema import Draft202012Validator


_BINDINGS_VALIDATOR = Draft202012Validator(
    json.loads(
        importlib.resources.files(__package__)
        .joinpath("rust-bindings.schema.json")
        .read_text()
    )
)


@dataclass(frozen=True)
class Bindings:
    """Immutable normalized Rust binding sidecar independent from the compiler runtime."""

    _value: Mapping[str, Any]

    @classmethod
    def from_dict(cls, value: Mapping[str, Any]) -> "Bindings":
        normalized = deepcopy(dict(value))
        errors = sorted(
            _BINDINGS_VALIDATOR.iter_errors(normalized),
            key=lambda error: tuple(str(part) for part in error.absolute_path),
        )
        if errors:
            raise ValueError(f"invalid Bindings: {errors[0].message}")
        for operation in normalized["operations"].values():
            operation.setdefault("stream", None)
        return cls(MappingProxyType(normalized))

    def to_dict(self) -> dict[str, Any]:
        return deepcopy(dict(self._value))
