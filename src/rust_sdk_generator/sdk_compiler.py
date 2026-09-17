"""Compile OpenAPI + normalized Rust bindings + policy into a Rust facade.

This is the generator-agnostic compiler boundary. Concrete generators are
adapter concerns and only produce :class:`RustBindingsIr`.
"""

from __future__ import annotations

from typing import Any

from . import sdk_emit
from . import sdk_frontend as frontend
from .sdk_ir import FacadeIr
from .sdk_model_lowering import resolve_models
from .sdk_operation_lowering import resolve_operations
from .sdk_raw_ir import RustBindingsIr
from .sdk_runtime import DEFAULT_RUNTIME, RustFacadeRuntime


GenerationError = frontend.GenerationError
OpenApiIndex = frontend.OpenApiIndex


def compile_ir(
    openapi: OpenApiIndex,
    bindings: RustBindingsIr,
    manifest: dict[str, Any],
) -> FacadeIr:
    """Validate and lower one explicit semantic facade policy."""
    try:
        ir = frontend.build_ir(openapi, bindings, manifest)
        return resolve_models(resolve_operations(ir, bindings), openapi, bindings)
    except ValueError as error:
        if isinstance(error, GenerationError):
            raise
        raise GenerationError(str(error)) from error


def _validate_runtime(ir: FacadeIr, runtime: RustFacadeRuntime) -> None:
    public_types = {ir.client_name, *(model.name for model in ir.models)}
    public_types.update(resource.name for resource in ir.resources)
    exported_runtime = set(runtime.error_exports) | {runtime.error_type}
    collisions = sorted(public_types & exported_runtime)
    if collisions:
        raise GenerationError(
            f"facade symbols collide with runtime exports: {collisions}"
        )
    resource_modules = {resource.module for resource in ir.resources}
    if runtime.error_module in resource_modules or runtime.error_module in {
        "mod",
        "facade_types",
    }:
        raise GenerationError(
            f"runtime error module collides with generated module: {runtime.error_module}"
        )


def compile_facade(
    openapi: OpenApiIndex,
    bindings: RustBindingsIr,
    manifest: dict[str, Any],
    *,
    runtime: RustFacadeRuntime = DEFAULT_RUNTIME,
) -> tuple[FacadeIr, dict[str, str]]:
    """Compile explicit semantics to resolved IR and deterministic Rust files."""
    ir = compile_ir(openapi, bindings, manifest)
    _validate_runtime(ir, runtime)
    return ir, sdk_emit.emit(ir, bindings.binding, runtime)
