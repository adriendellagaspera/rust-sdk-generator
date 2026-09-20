# rust-sdk-generator

Derive, validate and deterministically generate a Rust SDK from OpenAPI and a backend-neutral Rust Bindings contract. The generator does not fetch a particular API specification, own a consumer's runtime, or depend on an OpenAPI-to-Rust backend implementation.

This Cargo workspace contains two separate crates:

- `rust-sdk-generator`: OpenAPI indexing, Bindings consumption, public API derivation, structural validation, lowering and emission. It can be used with another backend that produces the same canonical Bindings contract.
- `openapi-to-rust-bindings`: an adapter for the pinned `openapi-to-rust` backend's generator-owned `binding-manifest.json`. It emits canonical Bindings v3 and can also validate existing v2/v3 sidecars.

The adapter is a producer of data, not a dependency of the root generator. Application-specific naming, source updates, integration runtime and publishing are owned by consumers.

## Start here

Requirements: Rust 1.88+; Python 3.11+ and Git for the standalone example. From the workspace root:

```sh
cargo test --workspace --all-targets --all-features
cargo run --quiet -p openapi-to-rust-bindings -- path/to/raw-output > rust-bindings.json

cargo run --quiet -p rust-sdk-generator -- derive \
  --openapi openapi.json --bindings rust-bindings.json \
  --surface surface.json --overrides overrides.json > derivation.json
```

`--surface` and `--overrides` are optional; without them derivation uses its default evidence and overrides. Inspect `derivation.json` before publishing: it contains both `definition` and an exhaustive `report.operations` map with `derived`, `overridden`, `excluded` or `rejected` outcomes and reasons. A rejected operation is not silently generated. Extract `definition` into `sdk-definition.json`, then run:

```sh
cargo run --quiet -p rust-sdk-generator -- generate \
  --openapi openapi.json --bindings rust-bindings.json \
  --definition sdk-definition.json --output src/sdk

cargo run --quiet -p rust-sdk-generator -- check-generated \
  --openapi openapi.json --bindings rust-bindings.json \
  --definition sdk-definition.json --output src/sdk
```

`generate` prints the public API inventory and publishes files to a dedicated output directory. `check` validates and compiles in memory, optionally writing an inventory; `check-generated` compares generated files with the directory without modifying it. See [contracts and CLI](docs/contracts.md) and [output publication](docs/output.md) before integrating either into a build.

For a pinned raw-backend → manifest → Bindings v3 → derivation → generation → independent Cargo consumer proof, including mock HTTP tests, use [the independent SDK example](examples/independent-sdk/README.md). It supplies its own API fixture and minimal runtime, not an existing consumer's artifacts.

## Documentation

- [Architecture and ownership](docs/architecture.md): data flow, provenance and responsibility boundaries.
- [Contracts and CLI](docs/contracts.md): versions, evidence, derivation report and error behavior.
- [Output safety](docs/output.md): markers, conflicts, publication, crash recovery and read-only checks.
- [Development and quality gates](docs/development.md): local verification, CI and review guidance.
- [Bindings adapter](openapi-to-rust-bindings/README.md): manifest precedence, compatibility and adapter API.

Consumer integrations should pin immutable revisions and explicitly review any contract or generated-output migration. A repository release does not automatically change consumer pins.
