# openapi-to-rust-bindings

Backend-specific adapter from `openapi-to-rust` output to the backend-neutral Bindings JSON contract consumed by `rust-sdk-generator`. This crate has no runtime dependency on the root generator.

The producer-delta audit in [`FORK_CAPABILITIES.json`](FORK_CAPABILITIES.json) records why the pinned fork and unmodified upstream emit different ordinary Rust, which fork-only behaviors are retained or retired, and the evidence for each disposition. It is intentionally separate from the adapter-supported integration envelope tracked in #155.

## Production input and explicit historical oracle

The production CLI takes two arguments: the generated directory containing
ordinary `client.rs` and `types.rs`, and the **exact effective** OpenAPI JSON
used by the pinned raw backend. This is the only default loading path:

```sh
cargo run --locked -p openapi-to-rust-bindings -- \
  path/to/raw-output effective-openapi.json > rust-bindings.json
```

The adapter inspects emitted Rust signatures, source identities and transport
behavior, cross-checks the effective spec and emits validated Bindings v3.
Missing or ambiguous identities, unemitted operations, unsupported stream ABIs
and invalid structure fail closed with `adapter.extract:` and specific
`extract.*` diagnostics. Supplying only the raw directory fails with
`adapter.input.effective_openapi_required`. A manifest or sidecar in the raw
directory is never consulted or used as a fallback.

**Historical oracle only:** `--legacy-metadata <generated-directory>`
explicitly invokes the retained `read_legacy_metadata(directory)` library function.
It prefers `binding-manifest.json` over `rust-bindings.json`, rejects invalid
metadata and does not inspect generated Rust. The manifest-specific
`COMPATIBILITY.json` remains the separate fork tracker pending #157; the
default upstream pin is [`DEFAULT_BACKEND.json`](DEFAULT_BACKEND.json).

## Structural inspection

`inspect_generated(directory)` parses ordinary generated `types.rs` and `client.rs` with a Rust AST:

```sh
cargo run --locked -p openapi-to-rust-bindings -- --inspect path/to/raw-output > structural-evidence.json
```

It reports public model fields and directly proven serde names, enums, aliases (including target-specific `cfg` alternatives), nested module paths, client construction/builders and exact public async method signatures with source locations. Unknown layouts, unsupported serde transforms and ambiguous client identity fail closed.

Structural evidence deliberately does not invent source-operation or transport semantics.

## Semantic inspection and manifest-free extraction

`inspect_semantics(directory, effective_openapi)` correlates emitted client methods to the exact effective OpenAPI and inspects their Rust bodies:

```sh
cargo run --locked -p openapi-to-rust-bindings -- \
  --inspect-semantics path/to/raw-output effective-openapi.json > semantic-evidence.json
```

The current supported path requires exact source identity from the emitted HTTP method and route evidence in the Rust method body, matched uniquely to the effective OpenAPI. Generated route documentation is used only as optional corroboration. Response representation and accepted success statuses are taken from emitted behavior and cross-checked against OpenAPI; a response media declaration alone never creates a call shape.

For call shapes whose required metadata is fully proven, `extract_bindings(directory, effective_openapi)` emits validated canonical Bindings v3:

```sh
cargo run --locked -p openapi-to-rust-bindings -- \
  --extract path/to/raw-output effective-openapi.json > rust-bindings.json
```

The extractor fails closed when source coverage is incomplete or required evidence is unavailable. Owned streams are accepted only when the emitted Rust exposes a complete native/WASM alias with matching item/error/lifetime ABI. Request discriminators and multipart helper semantics likewise require direct emitted-code evidence. Anonymous upstream stream return types, ambiguous aliases and unsupported discriminator projections remain explicit failures rather than naming guesses or replayed OpenAPI declarations.

## Explicit historical metadata loader

```sh
cargo run --locked -p openapi-to-rust-bindings -- --legacy-metadata path/to/raw-output > historical-rust-bindings.json
```

This explicitly invokes the historical `read_bindings` manifest/sidecar path. The library also exports `parse_binding_manifest(&str)`, `Bindings::from_value(Value)` and `Bindings::as_value()`. `MANIFEST_NAME` and `SIDECAR_NAME` expose the compatibility file names.

The manifest fixtures remain independent historical oracles. The scheduled/manual compatibility workflow and broader documentation audit belong to #157. The root generator, not this adapter, chooses the public SDK surface and applies consumer policy.

See [architecture](../docs/architecture.md) and [contracts](../docs/contracts.md) for the boundary with the root generator.
