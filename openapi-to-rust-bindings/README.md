# openapi-to-rust-bindings

Temporary compatibility shim between `gpu-cli/openapi-to-rust` bindings metadata and the backend-neutral Bindings v5 contract consumed by `rust-sdk-generator`.

Production uses unmodified upstream `openapi-to-rust`. The generator-owned `bindings.json` is authoritative for emitted symbols, exact signatures, source-operation identity and response planning. This crate exists only because metadata v1 does not yet expose every invocation detail required by the root generator.

## Production usage

The shim takes the generated directory and the **exact effective OpenAPI JSON** used by the pinned backend:

```sh
cargo run --locked -p openapi-to-rust-bindings -- \
  path/to/raw-output effective-openapi.json > rust-bindings.json
```

`--extract` is an equivalent explicit form.

There is no manifest, sidecar or source-only fallback. Missing or ambiguous evidence fails closed with `adapter.extract:` / `extract.*` diagnostics.

## Residual compatibility gaps

The target architecture is direct consumption of upstream `bindings.json` by `rust-sdk-generator`. Until upstream metadata exposes the remaining facts, this shim supplements metadata v1 with generated-code evidence for:

1. exact query/header Rust-parameter → wire-name mappings, recovered from generated request-building code;
2. fixed request effects used by generated method variants, recovered from generated request-building code;
3. backend-specific client construction/configuration roles, mapped from producer metadata until upstream gives them semantic roles;
4. live-stream ownership/transport details, proved from the exact producer-emitted type metadata until upstream exposes them structurally.

Everything already represented by upstream metadata should stay upstream-owned and must not be re-derived here.

The deletion criterion is explicit: once upstream metadata carries these residual facts, this crate should be removed rather than evolved into a second metadata model.

See [architecture](../docs/architecture.md) and [contracts](../docs/contracts.md).
