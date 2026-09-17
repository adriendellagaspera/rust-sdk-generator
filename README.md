# rust-sdk-generator

Backend-neutral Rust SDK generator. The root package consumes OpenAPI, normalized Rust Bindings, an explicit complete SDK description and runtime integration data, then emits deterministic idiomatic Rust source.

The standalone root package was migrated from the latest generic `rust-sdk-compiler` package branch in `adriendellagaspera/mistralai-rs` at `da67081e85b34859232b5b60451de72f44b0fb7e` (0.2.8). The sibling `openapi-to-rust-bindings/` component was migrated from its latest package branch at `37a7ca41174c9f3d80593dfb7bbccd77769847f6` (0.2.2). Those source revisions include the generic fixes merged after the historical extraction snapshots referenced by issue #7.

The root package deliberately has no dependency on `openapi-to-rust` or `openapi-to-rust-bindings`. `openapi-to-rust-bindings/` is a separately versioned distribution that may know `openapi-to-rust` source layout and compatibility details, but it communicates with the generator only through the normalized `Bindings` contract.

The compile-era public API is retained during the behavior-neutral bootstrap. Issue #1 evolves it to `derive()` / `generate()` with `PublicSdkSurface`, `SdkOverrides`, `SdkDefinition` and `DerivationReport`.

## Repository shape

```text
rust-sdk-generator/
├── pyproject.toml
├── uv.lock
├── src/rust_sdk_generator/
├── tests/
└── openapi-to-rust-bindings/
    ├── pyproject.toml
    ├── uv.lock
    ├── src/openapi_to_rust_bindings/
    ├── tests/
    └── scripts/check_backend_compat.py
```

CI validates the two distributions independently from built wheels and also runs a generic Menagerie integration proof:

```text
openapi-to-rust generated Rust fixture
              |
              v
openapi-to-rust-bindings
              |
              v
           Bindings
              |
              v
      rust-sdk-generator
              |
              v
   deterministic Rust SDK
```

## Bootstrap inventory

Moved to the root generator: backend-neutral OpenAPI indexing, the normalized `Bindings` contract, structural Rust-type reasoning, closed IR/lowering, deterministic Rust emission, runtime integration contracts, Bindings schema validation, and the Menagerie/Library/composed-OpenAPI generic fixtures and tests.

Moved to `openapi-to-rust-bindings/`: generated-source parsing, sidecar-first loading, `openapi-to-rust` source-layout assumptions, generic Menagerie/Library normalization fixtures, and the post-snapshot backend compatibility tracker. The tracker remains colocated with the component whose contract it protects; issue #8 evolves it to generator-owned binding metadata and defines source-parser retirement.

Intentionally left in `mistralai-rs`: Mistral OpenAPI/source tracking and overlays, official SDK surface extraction, Mistral taxonomy/naming evidence, the current auto-projection orchestration until issue #1 replaces it, Mistral runtime integration, consumer generation/compatibility/release gates, and all Mistral-specific coverage decisions.

A consumer should pin immutable standalone commits. `mistralai-rs#71` performs the first behavior-neutral consumer migration after issue #7 is complete; no historical package branch in `mistralai-rs` is a canonical package home after that migration.
