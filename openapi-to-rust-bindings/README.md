# openapi-to-rust-bindings

Backend-specific adapter from `openapi-to-rust` output to the backend-neutral Bindings JSON contract consumed by `rust-sdk-generator`. This crate has no runtime dependency on the root generator.

## Input precedence

The existing `read_bindings(directory)` path remains unchanged during the manifest-free migration. It reads `binding-manifest.json` when present and converts it to canonical Bindings v3. An invalid manifest is an error, even if a sidecar also exists. If there is no manifest, a validated `rust-bindings.json` sidecar is accepted (v2 or v3). When neither exists, loading fails.

The manifest path is a compatibility/oracle path, not the target backend contract. The manifest-free extractor described below consumes the ordinary generated Rust plus the exact effective OpenAPI independently. #151 owns switching the default user-facing path after end-to-end parity is established.

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

## Existing canonical loader

```sh
cargo run --quiet -p openapi-to-rust-bindings -- path/to/raw-output > rust-bindings.json
```

This invokes the existing `read_bindings` manifest/sidecar path. The library also exports `parse_binding_manifest(&str)`, `Bindings::from_value(Value)` and `Bindings::as_value()`. `MANIFEST_NAME` and `SIDECAR_NAME` expose the compatibility file names.

The pinned backend compatibility workflow and the manifest fixtures remain independent oracles while the manifest-free extractor is developed. The root generator, not this adapter, chooses the public SDK surface and applies consumer policy.

See [architecture](../docs/architecture.md) and [contracts](../docs/contracts.md) for the boundary with the root generator.
