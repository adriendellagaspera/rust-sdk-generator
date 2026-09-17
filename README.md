# rust-sdk-generator

Backend-neutral Rust SDK generator. The root crate consumes OpenAPI, normalized Rust Bindings, an explicit complete SDK definition and runtime integration data, then emits deterministic idiomatic Rust source.

The standalone root package was migrated from the latest generic `rust-sdk-compiler` package branch in `adriendellagaspera/mistralai-rs` at `da67081e85b34859232b5b60451de72f44b0fb7e` (0.2.8). The sibling `openapi-to-rust-bindings/` component was migrated from its latest package branch at `37a7ca41174c9f3d80593dfb7bbccd77769847f6` (0.2.2). Those source revisions include the generic fixes merged after the historical extraction snapshots referenced by issue #7.

The root crate deliberately has no dependency on `openapi-to-rust` or `openapi-to-rust-bindings`. `openapi-to-rust-bindings/` is a separately versioned Python distribution that may know `openapi-to-rust` source layout and compatibility details, but it communicates with the generator only through the normalized versioned `Bindings` JSON contract.

## Integration surfaces

The canonical generator is Rust and exposes two equivalent surfaces over the same implementation:

- the `rust_sdk_generator` library, including `generate(GenerateInput)`;
- the `rust-sdk-generator` CLI with deterministic `generate` and `check` commands.

For example:

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

Both commands emit the canonical API inventory as machine-readable JSON on stdout; `generate` additionally writes the generated Rust files. Automatic `derive()` remains intentionally reserved for the semantic expansion tracked by issue #1.

No Python runtime is required to run the root generator. Python remains only in the separately distributed `openapi-to-rust-bindings` compatibility adapter and repository policy tooling.

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
    ├── pyproject.toml
    ├── src/openapi_to_rust_bindings/
    ├── tests/
    └── scripts/check_backend_compat.py
```

CI validates the Rust crate and the adapter distribution independently, then runs a generic end-to-end proof:

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

The Rust test corpus also proves byte-for-byte behavior against the frozen generic Menagerie and Library fixtures and mechanically checks CLI/library equivalence.

## Ownership boundaries

Owned by the root Rust generator: backend-neutral OpenAPI indexing, the normalized `Bindings` consumer contract, structural Rust-type reasoning, closed IR/lowering, deterministic Rust emission, runtime integration contracts, complete-definition validation, canonical API inventory, and generic fixtures/tests.

Owned by `openapi-to-rust-bindings/`: generated-source parsing, sidecar-first loading, `openapi-to-rust` source-layout assumptions, the producer-side `Bindings` v2 validation model/schema, generic Menagerie/Library normalization fixtures, and the backend compatibility tracker. The adapter has no runtime dependency on the generator implementation. Issue #8 evolves this boundary toward generator-owned binding metadata and defines source-parser retirement.

Intentionally left in `mistralai-rs`: Mistral OpenAPI/source tracking and overlays, official SDK surface extraction, Mistral taxonomy/naming evidence, the current auto-projection orchestration until issue #1 replaces it, Mistral runtime integration, consumer generation/compatibility/release gates, and all Mistral-specific coverage decisions.

A consumer should pin immutable standalone commits. `mistralai-rs#71` performs the first behavior-neutral consumer migration after the extraction/migration gates are complete; no historical package branch in `mistralai-rs` is a canonical package home after that migration.
