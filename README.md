# rust-sdk-generator

Backend-neutral Rust SDK generator. The root package consumes OpenAPI, normalized Rust Bindings, an explicit complete SDK description and runtime integration data, then emits deterministic idiomatic Rust source.

The initial standalone behavior is migrated from the latest generic `rust-sdk-compiler` package branch in `adriendellagaspera/mistralai-rs` at `da67081e85b34859232b5b60451de72f44b0fb7e`. This includes the generic OpenAPI object-composition fixes merged after the historical extraction snapshot referenced by issue #7.

The package deliberately has no dependency on `openapi-to-rust` or `openapi-to-rust-bindings`. The sibling `openapi-to-rust-bindings/` component is imported separately and may depend on this package only through the normalized `Bindings` contract.

The compile-era public API is retained during the behavior-neutral bootstrap. Issue #1 evolves it to `derive()` / `generate()` with `PublicSdkSurface`, `SdkOverrides`, `SdkDefinition` and `DerivationReport`.

## Bootstrap inventory

Moved here: the complete backend-neutral generator package, its Menagerie/Library/composed-OpenAPI fixtures, Bindings schema validation, structural Rust-type logic, lowering, emission and generic tests from the current package branch.

Intentionally left in `mistralai-rs`: Mistral OpenAPI/source tracking, official SDK extraction, taxonomy/public naming evidence, current auto-projection orchestration, Mistral runtime integration and repository compatibility/release gates. Those responsibilities move only through their owning issues.
