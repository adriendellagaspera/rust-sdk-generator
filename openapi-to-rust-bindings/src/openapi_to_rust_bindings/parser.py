"""Read openapi-to-rust generated sources as rust-sdk-generator Bindings."""

from __future__ import annotations

from pathlib import Path
import re

from tree_sitter import Language, Node, Parser
import tree_sitter_rust

from .model import Bindings


class ParseError(ValueError):
    """Generated Rust cannot be normalized into the supported Bindings contract."""


def _text(source: bytes, node: Node | None) -> str:
    if node is None:
        raise ParseError("missing Rust AST node")
    return source[node.start_byte : node.end_byte].decode()


def _children(node: Node | None, kind: str) -> list[Node]:
    return [child for child in node.named_children if child.type == kind] if node else []


def _serde_rename(attributes: list[str]) -> str | None:
    matches = []
    for attribute in attributes:
        match = re.fullmatch(r'\#\[serde\(rename\s*=\s*"([^"]+)"\)\]', attribute)
        if match:
            matches.append(match.group(1))
    if len(matches) > 1:
        raise ParseError("multiple serde rename attributes on enum variant")
    return matches[0] if matches else None


def _split_generic(value: str) -> tuple[str, list[str]] | None:
    start = value.find("<")
    if start < 0 or not value.endswith(">"):
        return None
    constructor = value[:start].strip()
    body = value[start + 1 : -1]
    depth = 0
    part_start = 0
    parts: list[str] = []
    for index, character in enumerate(body):
        if character in "<([{":
            depth += 1
        elif character in ">)]}":
            depth -= 1
        elif character == "," and depth == 0:
            parts.append(body[part_start:index].strip())
            part_start = index + 1
    parts.append(body[part_start:].strip())
    return constructor, parts


def _stream_binding(success_type: str) -> dict[str, str] | None:
    outer = _split_generic(success_type)
    if outer is None:
        return None
    constructor, arguments = outer
    if constructor != "futures_util::stream::BoxStream" or len(arguments) != 2:
        return None
    event = _split_generic(arguments[1])
    if event is None or event[0] != "Result" or len(event[1]) != 2:
        return None
    return {
        "item_type": event[1][0],
        "error_type": event[1][1],
        "lifetime": arguments[0],
    }


def _bytes(source: bytes | str) -> bytes:
    return source if isinstance(source, bytes) else source.encode()


def parse_bindings(types_source: bytes | str, client_source: bytes | str) -> Bindings:
    """Parse generated ``types.rs`` and ``client.rs`` into normalized Bindings."""
    types_source = _bytes(types_source)
    client_source = _bytes(client_source)
    parser = Parser(Language(tree_sitter_rust.language()))

    roots: dict[str, Node] = {}
    symbol_paths: dict[str, str] = {}
    for label, source in (("types", types_source), ("client", client_source)):
        root = parser.parse(source).root_node
        if root.has_error:
            raise ParseError(f"invalid generated Rust {label} syntax")
        roots[label] = root
        for item in root.named_children:
            if item.type not in {"struct_item", "enum_item", "type_item"}:
                continue
            name = _text(source, item.child_by_field_name("name"))
            if name in symbol_paths:
                raise ParseError(f"ambiguous generated symbol {name}")
            symbol_paths[name] = f"crate::generated::{label}::{name}"

    structs: dict[str, list[dict[str, str]]] = {}
    enums: dict[str, list[dict[str, str | None]]] = {}
    aliases: dict[str, str] = {}

    for item in roots["types"].named_children:
        if item.type == "struct_item":
            name = _text(types_source, item.child_by_field_name("name"))
            fields = []
            for field in _children(item.child_by_field_name("body"), "field_declaration"):
                if not _children(field, "visibility_modifier"):
                    continue
                fields.append(
                    {
                        "name": _text(types_source, field.child_by_field_name("name")),
                        "type": _text(types_source, field.child_by_field_name("type")),
                    }
                )
            structs[name] = fields
        elif item.type == "enum_item":
            name = _text(types_source, item.child_by_field_name("name"))
            variants = []
            for variant in _children(item.child_by_field_name("body"), "enum_variant"):
                attributes = [
                    _text(types_source, child)
                    for child in variant.named_children
                    if child.type == "attribute_item"
                ]
                body = variant.child_by_field_name("body")
                variants.append(
                    {
                        "name": _text(types_source, variant.child_by_field_name("name")),
                        "payload": (
                            _text(types_source, body.named_children[0])
                            if body is not None and body.named_children
                            else None
                        ),
                        "wire_name": _serde_rename(attributes),
                    }
                )
            enums[name] = variants
        elif item.type == "type_item":
            name = _text(types_source, item.child_by_field_name("name"))
            aliases[name] = _text(types_source, item.child_by_field_name("type"))

    client_structs = [
        item
        for item in roots["client"].named_children
        if item.type == "struct_item"
    ]
    client_name = (
        _text(client_source, client_structs[0].child_by_field_name("name"))
        if client_structs
        else "ApiClient"
    )
    symbol_paths.setdefault(client_name, f"crate::generated::client::{client_name}")

    operations: dict[str, dict[str, object]] = {}
    for item in roots["client"].named_children:
        if item.type != "impl_item":
            continue
        impl_type = item.child_by_field_name("type")
        if impl_type is None or _text(client_source, impl_type) != client_name:
            continue
        for function in _children(item.child_by_field_name("body"), "function_item"):
            visibility = _children(function, "visibility_modifier")
            if not visibility:
                continue
            name = _text(client_source, function.child_by_field_name("name"))
            parameters = []
            for parameter in _children(function.child_by_field_name("parameters"), "parameter"):
                pattern = parameter.child_by_field_name("pattern")
                type_node = parameter.child_by_field_name("type")
                if pattern is None or type_node is None:
                    continue
                parameter_name = _text(client_source, pattern)
                if parameter_name in {"self", "&self", "&mut self"}:
                    continue
                parameters.append(
                    {
                        "name": parameter_name,
                        "type": _text(client_source, type_node),
                    }
                )
            return_type_node = function.child_by_field_name("return_type")
            return_type = _text(client_source, return_type_node) if return_type_node else "()"
            success_type = return_type
            result = _split_generic(return_type)
            if result is not None and result[0] == "Result" and result[1]:
                success_type = result[1][0]
            operations[name] = {
                "name": name,
                "parameters": parameters,
                "return_type": return_type,
                "success_type": success_type,
                "stream": _stream_binding(success_type),
            }

    value = {
        "schema_version": 2,
        "structs": structs,
        "enums": enums,
        "aliases": aliases,
        "operations": operations,
        "symbol_paths": symbol_paths,
        "binding": {
            "client": {
                "type_path": f"crate::generated::client::{client_name}",
                "constructor": "new",
                "api_key_builder": "with_api_key",
                "base_url_builder": "with_base_url",
            },
            "type_preludes": ["crate::generated::types::*"],
        },
    }
    try:
        return Bindings.from_dict(value)
    except ValueError as error:
        raise ParseError(str(error)) from error


def parse_directory(path: str | Path) -> Bindings:
    """Parse one generated output directory."""
    path = Path(path)
    return parse_bindings(
        (path / "types.rs").read_bytes(),
        (path / "client.rs").read_bytes(),
    )
