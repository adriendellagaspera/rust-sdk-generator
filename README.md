# rust-sdk-generator

Backend-neutral Rust SDK generator. The root crate consumes OpenAPI, normalized Rust Bindings, an explicit complete SDK definition and runtime integration data, then emits deterministic idiomatic Rust source.

The repository is a Cargo workspace with two deliberately independent Rust crates:

- `rust-sdk-generator`: the backend-neutral SDK generator;
- `openapi-to-rust-bindings`: the compatibility adapter from `openapi-to-rust` output to the versioned Bindings JSON contract.

The root generator has no dependency on `openapi-to-rust` or the adapter implementation. The adapter may know `openapi-to-rust` source layout and compatibility details, but communicates with the generator only through normalized Bindings JSON.

## Integration surfaces

The canonical generator exposes equivalent library and CLI surfaces over the same implementation:

```text
rust-sdk-generator generate \
  --openapi openapi.json \
  --bindings rust-bindings.json \
  --definition sdk-definition.json \
  --output src/sdk

rust-sdk-generator check \
  --openapi openapi.json \
  --bindings rust-bindings.json \
  --definition sdk-definition.json
```

The bindings adapter likewise exposes a Rust library and a small CLI:

```text
openapi-to-rust-bindings path/to/openapi-to-rust-output > rust-bindings.json
```

`read_bindings()` prefers a generator-owned `rust-bindings.json` sidecar and fails closed when it is invalid. Until issue #8 completes the generator-owned metadata migration, it retains the bounded compatibility fallback that normalizes generated `types.rs` and `client.rs`.

## Repository shape

```text
rust-sdk-generator/
├── Cargo.toml
├── Cargo.lock
├── src/*.rs
├── tests/
│   ├── fixtures/
│   ├── oracle/
│   └── rust_surface.rs
└── openapi-to-rust-bindings/
    ├── Cargo.toml
    ├── src/*.rs
    ├── rust-bindings.schema.json
    ├── tests/
    └── scripts/check_backend_compat.py
```

CI validates both Rust crates independently, then runs a generic end-to-end proof:

```text
openapi-to-rust generated Rust fixture
              |
              v
openapi-to-rust-bindings
              |
              v
     rust-bindings.json
              |
              v
 rust-sdk-generator CLI
              |
              v
   deterministic Rust SDK
```

## Ownership boundaries

Owned by the root Rust generator: backend-neutral OpenAPI indexing, the normalized `Bindings` consumer contract, structural Rust-type reasoning, closed IR/lowering, deterministic Rust emission, runtime integration contracts, complete-definition validation, canonical API inventory, and generic fixtures/tests.

Owned by `openapi-to-rust-bindings/`: generated-source parsing, sidecar-first loading, `openapi-to-rust` source-layout assumptions, producer-side Bindings v2 validation, generic Menagerie/Library normalization fixtures, and the backend compatibility tracker. The adapter has no runtime dependency on the generator implementation. Issue #8 evolves this boundary toward generator-owned binding metadata and defines source-parser retirement.

Intentionally left in `mistralai-rs`: Mistral OpenAPI/source tracking and overlays, official SDK surface extraction, Mistral taxonomy/naming evidence, Mistral runtime integration, consumer generation/compatibility/release gates, and all Mistral-specific coverage decisions.

Consumers should pin immutable standalone commits. Language/runtime migrations in this repository do not implicitly repin downstream consumers.
