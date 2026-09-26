# rust-sdk-generator

Derive and generate a Rust SDK from OpenAPI and a backend-neutral Bindings contract. This workspace contains the generator and `openapi-to-rust-bindings`, which proves canonical Bindings v3 from ordinary generated Rust and the exact effective OpenAPI. Consumer-specific source selection, runtime policy and publishing belong to the SDK repository.

## Start with your OpenAPI JSON

Requires Rust 1.88+, Cargo, Git and network access on the first run:

```sh
cargo run --locked --bin rust-sdk -- init \
  --openapi /path/to/your-api.json --output /path/to/your-sdk --name your-sdk
cargo run --locked --bin rust-sdk -- sync --crate /path/to/your-sdk --check
```

The CLI creates a standalone crate and a pinned, versioned recipe. Review the operation coverage and generated-file diff before accepting changes. See [own-API adoption](docs/own-api-sdk.md) for sync, ownership and supported shapes.

To run the fixed independent HTTP proof:

```sh
cargo run --locked --example independent-sdk-quickstart
```

The example generates a standalone SDK from its fixture, compiles it and exercises a local mock server. See [getting started](docs/getting-started.md) for its artifacts.

## Reference

- [Contracts and CLI](docs/contracts.md): versions, derivation outcomes and command behavior.
- [Architecture](docs/architecture.md): responsibility boundaries.
- [Output](docs/output.md): file ownership and safe publication.
- [Development](docs/development.md): local checks.
- [Bindings adapter](openapi-to-rust-bindings/README.md): extraction and supported evidence.

The lower-level `rust-sdk-generator` CLI accepts OpenAPI, Bindings and optional reviewed naming/override inputs. Its derivation report accounts for each operation; inspect rejected and excluded outcomes before publishing an SDK.
