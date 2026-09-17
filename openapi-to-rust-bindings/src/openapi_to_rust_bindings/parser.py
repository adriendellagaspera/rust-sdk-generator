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
        match = re.fullmatch(r'#\[serde\(rename\s*=\s*"([^"]+)"\)\]', attribute)
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
                fields.append({
                    "name": _text(types_source, field.child_by_field_name("name")),
                    "type": _text(types_source, field.child_by_field_name("type")),
                })
            structs[name] = fields
        elif item.type == "enum_item":
            name = _text(types_source, item.child_by_field_name("name"))
            variants = []
            pending_attributes: list[str] = []
            body = item.child_by_field_name("body")
            for child in body.named_children if body else ():
                if child.type == "attribute_item":
                    pending_attributes.append(_text(types_source, child))
                    continue
                if child.type != "enum_variant":
                    continue
                variant_name = _text(types_source, child.child_by_field_name("name"))
                attributes = pending_attributes + [
                    _text(types_source, attribute)
                    for attribute in _children(child, "attribute_item")
                ]
                pending_attributes = []
                variant_body = child.child_by_field_name("body")
                payload = None
                if variant_body and variant_body.type == "ordered_field_declaration_list":
                    payload_nodes = [
                        node for node in variant_body.named_children
                        if node.type != "attribute_item"
                    ]
                    if len(payload_nodes) != 1:
                        raise ParseError(f"enum {name}::{variant_name} is not unary")
                    payload = _text(types_source, payload_nodes[0])
                variants.append({
                    "name": variant_name,
                    "payload": payload,
                    "wire_name": _serde_rename(attributes),
                })
            enums[name] = variants
        elif item.type == "type_item":
            name = _text(types_source, item.child_by_field_name("name"))
            aliases[name] = _text(types_source, item.child_by_field_name("type"))

    operations: dict[str, dict[str, object]] = {}
    found_client = False
    for item in roots["client"].named_children:
        if item.type != "impl_item":
            continue
        if _text(client_source, item.child_by_field_name("type")) != "HttpClient":
            continue
        found_client = True
        for function in _children(item.child_by_field_name("body"), "function_item"):
            name = _text(client_source, function.child_by_field_name("name"))
            parameters = []
            parameter_list = function.child_by_field_name("parameters")
            for parameter in parameter_list.named_children if parameter_list else ():
                if parameter.type != "parameter":
                    continue
                identifier = next(
                    (
                        child for child in parameter.named_children
                        if child.type in {"identifier", "field_identifier"}
                    ),
                    None,
                )
                parameters.append({
                    "name": _text(client_source, identifier),
                    "type": _text(client_source, parameter.child_by_field_name("type")),
                })
            return_node = function.child_by_field_name("return_type")
            return_type = _text(client_source, return_node)
            success_type = ""
            if return_node and return_node.type == "generic_type":
                type_arguments = next(
                    (child for child in return_node.named_children if child.type == "type_arguments"),
                    None,
                )
                if type_arguments and type_arguments.named_children:
                    success_type = _text(client_source, type_arguments.named_children[0])
            operations[name] = {
                "name": name,
                "parameters": parameters,
                "return_type": return_type,
                "success_type": success_type,
                "stream": _stream_binding(success_type),
            }
    if client_source.strip() and not found_client:
        raise ParseError("openapi-to-rust HttpClient impl not found")

    return Bindings.from_dict({
        "schema_version": 2,
        "structs": structs,
        "enums": enums,
        "aliases": aliases,
        "operations": operations,
        "symbol_paths": symbol_paths,
        "binding": {
            "client": {
                "type_path": "crate::generated::client::HttpClient",
                "constructor": "new",
                "api_key_builder": "with_api_key",
                "base_url_builder": "with_base_url",
            },
            "type_preludes": ["crate::generated::types::*"],
        },
    })


def read_bindings(path: str | Path) -> Bindings:
    """Read an openapi-to-rust output directory into normalized Bindings."""
    path = Path(path)
    return parse_bindings((path / "types.rs").read_bytes(), (path / "client.rs").read_bytes())
