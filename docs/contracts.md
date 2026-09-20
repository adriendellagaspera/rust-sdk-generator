# Contracts and CLI

The Rust library exports `derive(DeriveInput) -> Result<Derivation, DerivationError>` and `generate(GenerateInput) -> Result<GeneratedSdk, GenerationError>`. The CLI reads JSON files for the same typed inputs and writes deterministic JSON to stdout; it does not orchestrate the raw backend or construct a consumer runtime.

## Versioned inputs and outputs

| Contract | Version / behavior |
| --- | --- |
| `OpenApi` | Published OpenAPI JSON supplied separately by the consumer |
| `Bindings` | v3 is the structured manifest adapter's output; the root generator and adapter also validate legacy v2 inputs |
| `PublicSdkSurface` | v1: optional `client` and source operation ID → public-path list under `operations` |
| `SdkOverrides` | v1: explicit exclusions and bounded operation overrides |
| `SdkDefinition` | v2: complete public client, models and resources accepted by generation |
| `DerivationReport` | v1: outcome and machine-readable reason per relevant source operation |
| `Runtime` | Consumer-owned integration paths, with defaults; no separate schema-version field |
| `GeneratedSdk` | Generated-file map and public API inventory produced by the same lowering pass |

Canonical Bindings include Rust symbols, qualified paths, call shapes and, in v3, source-operation identity, representation, statuses, discriminators, stream ABI and field wire names. The adapter's [JSON schema](../openapi-to-rust-bindings/rust-bindings.schema.json) and `src/contracts.rs` / `src/validation.rs` define the accepted structures; do not reconstruct them from generated Rust source. `openapi-to-rust-bindings/COMPATIBILITY.json` pins the backend revision independently of the root generator.

A `PublicSdkSurface` entry may list multiple paths for a single source operation (for example a buffered and streaming view). Explicit `response_representations` choose among structurally supported variants; `request_overrides` and exclusions are applied only through the validated generic contract. An override of a rejected operation is an error, not a way to bypass structural validation.

The report records `derived`, `overridden`, `excluded` and `rejected` with a reason for every relevant OpenAPI operation. Check it explicitly against the consumer's expected operation inventory. A successful `derive` invocation does **not** mean every source operation was generated. Extract the top-level `definition` value from the returned JSON before passing it to generation; do not pass the entire derivation object to `--definition`.

## CLI commands

```text
rust-sdk-generator derive --openapi FILE --bindings FILE [--surface FILE] [--overrides FILE]
rust-sdk-generator generate --openapi FILE --bindings FILE --definition FILE --output DIR [--runtime FILE] [--inventory FILE]
rust-sdk-generator check --openapi FILE --bindings FILE --definition FILE [--runtime FILE] [--inventory FILE]
rust-sdk-generator check-generated --openapi FILE --bindings FILE --definition FILE --output DIR [--runtime FILE]
```

`derive` writes the derivation and report. `generate` writes generated files and prints the inventory; its optional `--inventory` writes a separate inventory file. `check` validates/compiles in memory, prints the inventory, and can write it to `--inventory`; it does not compare files on disk. `check-generated` is read-only and prints JSON arrays `missing`, `changed`, `extra` and `conflicts`; it rejects `--inventory` because that would write a file.

Exit code 0 indicates a successful command (or a clean comparison), 1 indicates a stale `check-generated` comparison and 2 indicates a CLI/input/IO/generation failure. CLI errors are JSON objects on stderr containing `code`, `message` and `path`. For safe output-directory behavior and crash recovery, read [output publication](output.md).
