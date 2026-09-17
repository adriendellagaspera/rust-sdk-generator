"""Immutable normalized Rust bindings consumed by facade lowering.

The compiler consumes this IR and does not care how it was obtained. Generator-
specific adapters are responsible for mapping their output into these types.
"""

from __future__ import annotations

from dataclasses import dataclass
from types import MappingProxyType
from typing import Any, Mapping

from .rust_types import RustType, parse_type


class RawIrError(ValueError):
    """The normalized Rust binding contract is missing, ambiguous, or malformed."""


@dataclass(frozen=True)
class RawField:
    name: str
    syntax: RustType

    @property
    def type(self) -> str:
        return self.syntax.spelling


@dataclass(frozen=True)
class RawVariant:
    name: str
    payload: str | None
    wire_name: str | None = None


@dataclass(frozen=True)
class RawParameter:
    name: str
    syntax: RustType

    @property
    def type(self) -> str:
        return self.syntax.spelling


@dataclass(frozen=True)
class RawStreamBinding:
    """Semantic stream contract independent from its concrete Rust wrapper type."""

    item_type: str
    error_type: str
    lifetime: str


@dataclass(frozen=True)
class RawOperation:
    name: str
    parameters: tuple[RawParameter, ...]
    return_type: str
    success_type: str
    stream: RawStreamBinding | None = None


@dataclass(frozen=True)
class RawClientBinding:
    """How the facade reaches and configures the generated/raw Rust client."""

    type_path: str
    constructor: str
    api_key_builder: str
    base_url_builder: str


@dataclass(frozen=True)
class RawBindingLayout:
    """Rust paths needed by emission, independent from generator file layout."""

    client: RawClientBinding
    type_preludes: tuple[str, ...] = ()


@dataclass(frozen=True)
class RawIr:
    """Normalized Rust binding shape, independent from the producing generator."""

    _structs: tuple[tuple[str, tuple[RawField, ...]], ...]
    _enums: tuple[tuple[str, tuple[RawVariant, ...]], ...]
    _aliases: tuple[tuple[str, RustType], ...]
    _operations: tuple[tuple[str, RawOperation], ...]
    _symbol_paths: tuple[tuple[str, str], ...]
    binding: RawBindingLayout

    @classmethod
    def from_parts(
        cls,
        *,
        structs: Mapping[str, tuple[RawField, ...]],
        enums: Mapping[str, tuple[RawVariant, ...]],
        aliases: Mapping[str, RustType],
        operations: Mapping[str, RawOperation],
        symbol_paths: Mapping[str, str],
        binding: RawBindingLayout,
    ) -> "RawIr":
        declared = set(structs) | set(enums) | set(aliases)
        unknown_paths = sorted(declared - set(symbol_paths))
        if unknown_paths:
            raise RawIrError(f"missing Rust symbol paths: {unknown_paths}")
        return cls(
            tuple(sorted(structs.items())),
            tuple(sorted(enums.items())),
            tuple(sorted(aliases.items())),
            tuple(sorted(operations.items())),
            tuple(sorted(symbol_paths.items())),
            binding,
        )

    @classmethod
    def from_dict(cls, value: Mapping[str, Any]) -> "RawIr":
        """Load the versioned machine-readable Rust bindings representation."""
        if value.get("schema_version") != 2:
            raise RawIrError("unsupported raw IR sidecar version")
        try:
            structs = {
                name: tuple(
                    RawField(field["name"], parse_type(field["type"])) for field in fields
                )
                for name, fields in value["structs"].items()
            }
            enums = {
                name: tuple(
                    RawVariant(
                        variant["name"], variant.get("payload"), variant.get("wire_name")
                    )
                    for variant in variants
                )
                for name, variants in value["enums"].items()
            }
            aliases = {
                name: parse_type(type_name) for name, type_name in value["aliases"].items()
            }
            operations = {}
            for name, operation in value["operations"].items():
                stream_value = operation.get("stream")
                stream = (
                    RawStreamBinding(
                        stream_value["item_type"],
                        stream_value["error_type"],
                        stream_value["lifetime"],
                    )
                    if stream_value is not None
                    else None
                )
                operations[name] = RawOperation(
                    operation["name"],
                    tuple(
                        RawParameter(parameter["name"], parse_type(parameter["type"]))
                        for parameter in operation["parameters"]
                    ),
                    operation["return_type"],
                    operation["success_type"],
                    stream,
                )
            symbol_paths = {
                name: path for name, path in value["symbol_paths"].items()
            }
            client_value = value["binding"]["client"]
            binding = RawBindingLayout(
                RawClientBinding(
                    client_value["type_path"],
                    client_value["constructor"],
                    client_value["api_key_builder"],
                    client_value["base_url_builder"],
                ),
                tuple(value["binding"].get("type_preludes", ())),
            )
        except (KeyError, TypeError, AttributeError, ValueError) as error:
            raise RawIrError("malformed raw IR sidecar") from error
        return cls.from_parts(
            structs=structs,
            enums=enums,
            aliases=aliases,
            operations=operations,
            symbol_paths=symbol_paths,
            binding=binding,
        )

    def to_dict(self) -> dict[str, Any]:
        """Return the deterministic versioned Rust bindings representation."""
        return {
            "schema_version": 2,
            "structs": {
                name: [{"name": field.name, "type": field.type} for field in fields]
                for name, fields in self._structs
            },
            "enums": {
                name: [
                    {
                        "name": variant.name,
                        "payload": variant.payload,
                        "wire_name": variant.wire_name,
                    }
                    for variant in variants
                ]
                for name, variants in self._enums
            },
            "aliases": {name: syntax.spelling for name, syntax in self._aliases},
            "operations": {
                name: {
                    "name": operation.name,
                    "parameters": [
                        {"name": parameter.name, "type": parameter.type}
                        for parameter in operation.parameters
                    ],
                    "return_type": operation.return_type,
                    "success_type": operation.success_type,
                    "stream": (
                        {
                            "item_type": operation.stream.item_type,
                            "error_type": operation.stream.error_type,
                            "lifetime": operation.stream.lifetime,
                        }
                        if operation.stream is not None
                        else None
                    ),
                }
                for name, operation in self._operations
            },
            "symbol_paths": dict(self._symbol_paths),
            "binding": {
                "client": {
                    "type_path": self.binding.client.type_path,
                    "constructor": self.binding.client.constructor,
                    "api_key_builder": self.binding.client.api_key_builder,
                    "base_url_builder": self.binding.client.base_url_builder,
                },
                "type_preludes": list(self.binding.type_preludes),
            },
        }

    @property
    def structs(self) -> Mapping[str, tuple[RawField, ...]]:
        return MappingProxyType(dict(self._structs))

    @property
    def enums(self) -> Mapping[str, tuple[RawVariant, ...]]:
        return MappingProxyType(dict(self._enums))

    @property
    def aliases(self) -> Mapping[str, RustType]:
        return MappingProxyType(dict(self._aliases))

    @property
    def operations(self) -> Mapping[str, RawOperation]:
        return MappingProxyType(dict(self._operations))

    @property
    def symbol_paths(self) -> Mapping[str, str]:
        return MappingProxyType(dict(self._symbol_paths))

    def fields(self, name: str) -> tuple[RawField, ...]:
        try:
            return self.structs[name]
        except KeyError as error:
            raise RawIrError(f"raw Rust struct not found: {name}") from error

    def variants(self, name: str) -> tuple[RawVariant, ...]:
        try:
            return self.enums[name]
        except KeyError as error:
            raise RawIrError(f"raw Rust enum not found: {name}") from error

    def operation(self, name: str) -> RawOperation:
        try:
            return self.operations[name]
        except KeyError as error:
            raise RawIrError(f"raw Rust operation not found: {name}") from error

    def qualified_type(self, spelling: str) -> str:
        paths = self.symbol_paths

        def render(syntax: RustType) -> str:
            if syntax.kind == "generic_type":
                return (
                    f"{syntax.constructor}<"
                    + ", ".join(render(argument) for argument in syntax.arguments)
                    + ">"
                )
            return paths.get(syntax.spelling, syntax.spelling)

        return render(parse_type(spelling))


# Kept as a semantic alias while downstream consumers migrate terminology.
RustBindingsIr = RawIr
