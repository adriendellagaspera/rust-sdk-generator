# rust-sdk-generator

Turn an OpenAPI description into a standalone Rust SDK crate. The CLI derives a public API,
generates Rust files and reports an outcome for every source operation, so unsupported or
excluded operations are visible before publication.

This workspace contains the backend-neutral `rust-sdk-generator` and a concrete
`openapi-to-rust-bindings` adapter. The adapter extracts canonical Bindings v3 from an
unmodified backend's generated Rust and its effective OpenAPI; the root generator consumes
that contract. SDK repositories own their API source, runtime policy and releases.

## Generate an SDK for your API

From this checkout, with Rust 1.88+, Cargo, Git and first-run network access:

```sh
cargo run --locked --bin rust-sdk -- init \
  --openapi /path/to/your-api.json --output /path/to/your-sdk --name your-sdk
cargo run --locked --bin rust-sdk -- sync --crate /path/to/your-sdk --check
```

`init` creates a separate compilable SDK crate, copies the API source and pins a versioned
generation recipe. After editing that crate's `openapi.json`, `sync --check` shows operation
coverage and a file-level diff without changing the generated files. Review the outcomes
before accepting a changed inventory. The supported OpenAPI envelope is bounded, and the
minimal generated runtime is not a production authentication or error policy. See
[using your own API](docs/own-api-sdk.md) for supported shapes and ownership.

## Try the self-contained example

```sh
cargo run --locked --example independent-sdk-quickstart
```

This command uses a fixed notebook API fixture: it generates a standalone SDK, compiles it
and checks it against a local mock HTTP server. It does not exercise your API. See the
[getting-started guide](docs/getting-started.md) for the output and next steps.

## Go deeper

- [Contracts and CLI](docs/contracts.md): Bindings, derivation outcomes and lower-level commands.
- [Architecture](docs/architecture.md): data flow and responsibility boundaries.
- [Output safety](docs/output.md): file ownership and publication.
- [Development](docs/development.md): local verification.
- [Bindings adapter](openapi-to-rust-bindings/README.md): extraction and supported evidence.

The lower-level CLI accepts OpenAPI, Bindings and optional reviewed naming or override inputs.
Its derivation report accounts for each source operation; inspect rejected and excluded
outcomes before publishing a consumer SDK.
