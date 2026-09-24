# rust-sdk-generator

Derive, validate and deterministically generate a Rust SDK from OpenAPI and a backend-neutral Rust Bindings contract. The generator does not fetch a particular API specification, own a consumer's runtime, or depend on an OpenAPI-to-Rust backend implementation.

This Cargo workspace contains two separate crates:

- `rust-sdk-generator`: OpenAPI indexing, Bindings consumption, public API derivation, structural validation, lowering and emission. It can be used with another backend that produces the same canonical Bindings contract.
- `openapi-to-rust-bindings`: an adapter extracting canonical Bindings v3 from unmodified upstream's ordinary generated Rust plus the exact effective OpenAPI JSON. Historical manifest/sidecar reading requires an explicit oracle-only CLI flag.

The adapter is a producer of data, not a dependency of the root generator. Application-specific naming, source updates, integration runtime and publishing are owned by consumers.

## Start a new standalone SDK from your own OpenAPI JSON

From the generator checkout with Rust 1.88+, Cargo, Git and first-run network access:

```sh
cargo run --locked --bin rust-sdk -- init \
  --openapi /path/to/your-api.json --output /path/to/your-sdk --name your-sdk
cargo run --locked --bin rust-sdk -- sync --crate /path/to/your-sdk --check
```

The Rust-native `rust-sdk` CLI pins the unmodified upstream backend and effective source,
emits canonical Bindings v3, derives an exhaustive report, and assembles a
standalone compilable SDK crate with an explicit versioned recipe. After changing
the copied source at `/path/to/your-sdk/openapi.json`, inspect sync's full
coverage report and file-level freshness diff; use `--accept-coverage` to
reviewably accept a changed operation inventory. The minimal runtime is **not**
a production auth/error policy, and the supported envelope is deliberately
bounded. See [own-API onboarding and ownership](docs/own-api-sdk.md).

## Quickstart: fixed independent notebook proof

From a clean repository checkout, with Rust 1.88+, Cargo, Git and first-run network access:

```sh
cargo run --locked --example independent-sdk-quickstart
```

This native Rust entry point checks out the immutable raw-backend revision pinned
in `openapi-to-rust-bindings/DEFAULT_BACKEND.json`, builds the backend, adapter and
generator, then derives, generates, compiles and HTTP-tests a standalone notebook
SDK. It prints the output directory. No Python or downstream consumer checkout
is needed. [Follow the quickstart and adapt your own API](docs/getting-started.md).

The example is turnkey for its **fixed notebook fixture**; the independent
own-API `rust-sdk` CLI above is the separately tested general onboarding path
within the declared supported OpenAPI envelope.

## Start here

The underlying CLI remains available for integration. With Rust 1.88+ and a
normalized Bindings JSON file, run from the workspace root:

```sh
cargo test --workspace --all-targets --all-features
cargo run --locked -p openapi-to-rust-bindings -- path/to/raw-output effective-openapi.json > rust-bindings.json

cargo run --quiet -p rust-sdk-generator -- derive \
  --openapi openapi.json --bindings rust-bindings.json \
  --surface surface.json --overrides overrides.json \
  --definition-output sdk-definition.json > derivation.json
```

`--surface` and `--overrides` are optional; without them derivation uses its default evidence and overrides. Inspect `derivation.json` before publishing: it contains both `definition` and an exhaustive `report.operations` map with `derived`, `overridden`, `excluded` or `rejected` outcomes and reasons. A rejected operation is not silently generated. The optional
`--definition-output` writes the derived definition directly, without an
external JSON extraction script. Then run:

```sh
cargo run --quiet -p rust-sdk-generator -- generate \
  --openapi openapi.json --bindings rust-bindings.json \
  --definition sdk-definition.json --output src/sdk

cargo run --quiet -p rust-sdk-generator -- check-generated \
  --openapi openapi.json --bindings rust-bindings.json \
  --definition sdk-definition.json --output src/sdk
```

`generate` prints the public API inventory and publishes files to a dedicated output directory. `check` validates and compiles in memory, optionally writing an inventory; `check-generated` compares generated files with the directory without modifying it. See [contracts and CLI](docs/contracts.md) and [output publication](docs/output.md) before integrating either into a build.

For implementation details and failure diagnostics of the native independent
proof, see [the independent SDK example](examples/independent-sdk/README.md).
It supplies its own API fixture and minimal runtime, not an existing consumer's artifacts.

## Documentation

- [Getting started](docs/getting-started.md): native Rust quickstart, artifacts and adoption limits.
- [Architecture and ownership](docs/architecture.md): data flow, provenance and responsibility boundaries.
- [Contracts and CLI](docs/contracts.md): versions, evidence, derivation report and error behavior.
- [Output safety](docs/output.md): markers, conflicts, publication, crash recovery and read-only checks.
- [Development and quality gates](docs/development.md): local verification, CI and review guidance.
- [Bindings adapter](openapi-to-rust-bindings/README.md): manifest-free extraction, historical oracle, support envelope and diagnostics.

Consumer integrations should pin immutable revisions and explicitly review any contract or generated-output migration. A repository release does not automatically change consumer pins.
