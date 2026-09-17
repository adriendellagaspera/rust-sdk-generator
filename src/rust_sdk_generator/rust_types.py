"""Small structural parser for normalized Rust type spellings.

This parser intentionally understands only the generic structure the compiler
needs (`Option<T>`, `Vec<T>`, `Result<T, E>`, qualified generic types, etc.).
Everything else is retained as an opaque leaf. Parsing generated Rust source is
an adapter concern; the compiler must not depend on a Rust syntax-tree parser.
"""

from __future__ import annotations

from dataclasses import dataclass
from functools import lru_cache


@dataclass(frozen=True)
class Type:
    kind: str
    spelling: str
    constructor: str | None = None
    arguments: tuple["Type", ...] = ()

    def unary(self, constructor: str) -> "Type | None":
        if (
            self.kind == "generic_type"
            and self.constructor == constructor
            and len(self.arguments) == 1
        ):
            return self.arguments[0]
        return None


# Private compatibility name for compiler modules written before Type became public.
RustType = Type


def _validate_type_spelling(value: str) -> None:
    """Reject statement boundaries while retaining unknown type syntax as opaque."""
    angles = parentheses = brackets = braces = 0
    in_string = False
    escaped = False
    for char in value:
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
            continue
        if char == '"':
            in_string = True
        elif char == "<":
            angles += 1
        elif char == ">":
            angles -= 1
        elif char == "(":
            parentheses += 1
        elif char == ")":
            parentheses -= 1
        elif char == "[":
            brackets += 1
        elif char == "]":
            brackets -= 1
        elif char == "{":
            braces += 1
        elif char == "}":
            braces -= 1
        elif char == ";" and angles == parentheses == brackets == braces == 0:
            raise ValueError("unexpected Rust statement boundary in type")
        if min(angles, parentheses, brackets, braces) < 0:
            raise ValueError("unbalanced Rust type delimiters")
    if in_string or any((angles, parentheses, brackets, braces)):
        raise ValueError("unbalanced Rust type delimiters")


def _split_arguments(value: str) -> tuple[str, ...]:
    parts: list[str] = []
    start = 0
    angles = parentheses = brackets = braces = 0
    in_string = False
    escaped = False
    for index, char in enumerate(value):
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
            continue
        if char == '"':
            in_string = True
        elif char == "<":
            angles += 1
        elif char == ">":
            angles -= 1
        elif char == "(":
            parentheses += 1
        elif char == ")":
            parentheses -= 1
        elif char == "[":
            brackets += 1
        elif char == "]":
            brackets -= 1
        elif char == "{":
            braces += 1
        elif char == "}":
            braces -= 1
        elif (
            char == ","
            and angles == parentheses == brackets == braces == 0
        ):
            part = value[start:index].strip()
            if not part:
                raise ValueError("empty Rust generic argument")
            parts.append(part)
            start = index + 1
        if min(angles, parentheses, brackets, braces) < 0:
            raise ValueError("unbalanced Rust type delimiters")
    if in_string or any((angles, parentheses, brackets, braces)):
        raise ValueError("unbalanced Rust type delimiters")
    final = value[start:].strip()
    if not final:
        raise ValueError("empty Rust generic argument")
    parts.append(final)
    return tuple(parts)


def _outer_generic(spelling: str) -> tuple[str, str] | None:
    """Return `(constructor, arguments)` when `<...>` spans the whole type."""
    depth = 0
    first = None
    in_string = False
    escaped = False
    for index, char in enumerate(spelling):
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
            continue
        if char == '"':
            in_string = True
            continue
        if char == "<":
            if depth == 0:
                first = index
            depth += 1
        elif char == ">":
            depth -= 1
            if depth < 0:
                raise ValueError("unbalanced Rust generic delimiters")
            if depth == 0 and index != len(spelling) - 1:
                return None
    if depth != 0 or in_string:
        raise ValueError("unbalanced Rust generic delimiters")
    if first is None:
        return None
    constructor = spelling[:first].strip()
    if not constructor or any(char.isspace() for char in constructor):
        return None
    return constructor, spelling[first + 1 : -1]


@lru_cache(maxsize=4096)
def parse_type(spelling: str) -> Type:
    spelling = spelling.strip()
    if not spelling:
        raise ValueError("empty Rust type")
    _validate_type_spelling(spelling)
    generic = _outer_generic(spelling)
    if generic is None:
        return Type("opaque_type", spelling)
    constructor, arguments = generic
    parsed = tuple(parse_type(argument) for argument in _split_arguments(arguments))
    return Type("generic_type", spelling, constructor, parsed)
