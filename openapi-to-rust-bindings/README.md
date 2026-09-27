# openapi-to-rust-bindings

Backend-specific adapter from `openapi-to-rust` output to the backend-neutral Bindings JSON contract consumed by `rust-sdk-generator`. This crate has no runtime dependency on the root generator.

`FORK_CAPABILITIES.json` is retained as historical evidence from the former fork era. Production generation now pins unmodified `gpu-cli/openapi-to-rust`; the fork is not a production backend.

## Production input and explicit historical oracle

The production CLI takes two arguments: the generated directory containing
ordinary `client.rs`, `types.rs` and upstream `bindings.json`, and the **exact effective** OpenAPI JSON
used by the pinned raw backend. `bindings.json` is the primary source for emitted symbols,
signatures, source-operation identity and response planning; Rust/body inspection remains a
complementary proof only for contract fields not exposed by metadata v1.

```sh
cargo run --locked -p openapi-to-rust-bindings -- \
  path/to/raw-output effective-openapi.json > rust-bindings.json
```

The adapter validates upstream metadata against emitted Rust and the effective spec, adds the
remaining directly observable wire/discriminator/stream evidence, and emits validated Bindings v3.
Missing or ambiguous identities, unemitted operations, unsupported stream ABIs
and invalid structure fail closed with `adapter.extract:` and specific
`extract.*` diagnostics. Supplying only the raw directory fails with
`adapter.input.effective_openapi_required`. A manifest or sidecar in the raw
directory is never consulted or used as a fallback.

**Historical oracle only:** `--legacy-metadata <generated-directory>`
explicitly invokes the retained `read_legacy_metadata(directory)` library function.
It prefers `binding-manifest.json` over `rust-bindings.json`, rejects invalid
metadata and does not inspect generated Rust. Production compatibility is tracked in [`COMPATIBILITY.json`](COMPATIBILITY.json) and must match the default upstream pin in [`DEFAULT_BACKEND.json`](DEFAULT_BACKEND.json). The manifest-era fork tracker is isolated in [`LEGACY_COMPATIBILITY.json`](LEGACY_COMPATIBILITY.json) and is never a production fallback.

## Structural inspection

`inspect_generated(directory)` parses ordinary generated `types.rs` and `client.rs` with a Rust AST:

```sh
cargo run --locked -p openapi-to-rust-bindings -- --inspect path/to/raw-output > structural-evidence.json
```

It reports public model fields and directly proven serde names, enums, aliases (including target-specific `cfg` alternatives), nested module paths, client construction/builders and exact public async method signatures with source locations. Unknown layouts, unsupported serde transforms and ambiguous client identity fail closed.

Structural evidence deliberately does not invent source-operation or transport semantics.

## Semantic inspection and explicit source-only proof path

`inspect_semantics(directory, effective_openapi)` correlates emitted client methods to the exact effective OpenAPI and inspects their Rust bodies:

```sh
cargo run --locked -p openapi-to-rust-bindings -- \
  --inspect-semantics path/to/raw-output effective-openapi.json > semantic-evidence.json
```

`inspect_semantics` and `extract_bindings_from_rust` are retained as explicit proof/test tools for evidence that can be recovered from emitted Rust. They are not the production loader. Production `extract_bindings` starts from upstream `bindings.json`, cross-checks the emitted Rust and exact effective OpenAPI, then supplements only the evidence metadata v1 does not expose.

The production CLI invokes metadata-backed `extract_bindings(directory, effective_openapi)` and emits validated canonical Bindings v3:

```sh
cargo run --locked -p openapi-to-rust-bindings -- \
  --extract path/to/raw-output effective-openapi.json > rust-bindings.json
```

The extractor fails closed when metadata/source/output evidence disagrees or required evidence is unavailable. Request discriminators and exact query/header wire mappings still require direct emitted-code evidence. The current Bindings v3 stream ABI still requires a named native/WASM alias; support for upstream anonymous owned streams is tracked separately in #197.

## Explicit historical metadata loader

```sh
cargo run --locked -p openapi-to-rust-bindings -- --legacy-metadata path/to/raw-output > historical-rust-bindings.json
```

This explicitly invokes the historical `read_legacy_metadata` manifest/sidecar path. The library also exports `parse_binding_manifest(&str)`, `Bindings::from_value(Value)` and `Bindings::as_value()`. `MANIFEST_NAME` and `SIDECAR_NAME` expose the compatibility file names.

The manifest fixtures remain independent historical oracles. Scheduled and ordinary compatibility exercise the production upstream boundary; the manifest-era workflow is manual/optional. The root generator, not this adapter, chooses the public SDK surface and applies consumer policy.

See [architecture](../docs/architecture.md) and [contracts](../docs/contracts.md) for the boundary with the root generator.
