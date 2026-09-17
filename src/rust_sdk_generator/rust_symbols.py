"""Deterministic public symbol allocation with fail-closed naming policy.

Explicit product names are never silently suffixed: a collision requires an
overlay decision so an upstream addition cannot rename an existing API.
"""
from dataclasses import dataclass

KEYWORDS = frozenset("as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while abstract become box do final gen macro override priv typeof unsized virtual yield try union".split())


def field_identifier(name: str) -> str:
    """Escape protocol-owned keywords without silently changing their identity."""
    name = name.removeprefix("r#")
    if name in {"self", "Self", "super", "crate", "_"}:
        raise ValueError(f"Rust cannot represent field identifier {name!r}; explicit mapping required")
    return "r#" + name if name in KEYWORDS else name


@dataclass(frozen=True)
class Symbol:
    name: str
    namespace: str
    source: str
    module: str


class SymbolProvider:
    def __init__(self):
        self.symbols: dict[tuple[str, str], Symbol] = {}

    def claim(self, name: str, namespace: str, source: str, module: str = "") -> Symbol:
        if not name.isascii() or not name.isidentifier() or name == "_" or name in KEYWORDS:
            raise ValueError(f"invalid/reserved Rust symbol {name!r} at {source}; choose an explicit semantic name")
        key = (namespace, name)
        if key in self.symbols:
            previous = self.symbols[key]
            raise ValueError(f"Rust symbol collision {namespace}::{name}: {previous.source} vs {source}")
        symbol = Symbol(name, namespace, source, module)
        self.symbols[key] = symbol
        return symbol
