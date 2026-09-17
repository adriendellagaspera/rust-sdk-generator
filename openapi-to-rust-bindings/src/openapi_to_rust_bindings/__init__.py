"""Normalize openapi-to-rust output into rust-sdk-generator Bindings."""

from .model import Bindings
from .parser import ParseError, parse_bindings
from .reader import read_bindings

__version__ = "0.2.2"

__all__ = ["Bindings", "ParseError", "parse_bindings", "read_bindings"]
