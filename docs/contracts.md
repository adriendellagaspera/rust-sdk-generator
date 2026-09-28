# Contracts and CLI

The Rust library exports `derive(DeriveInput) -> Result<Derivation, DerivationError>` and `generate(GenerateInput) -> Result<GeneratedSdk, GenerationError>`. The CLI reads JSON files for the same typed inputs and writes deterministic JSON to stdout; it does not orchestrate the raw backend or construct a consumer runtime.

## Versioned inputs and outputs

| Contract | Version / behavior |
| --- | --- |
| `OpenApi` | Published OpenAPI JSON supplied separately by the consumer |
| `Bindings` | v4 is the canonical metadata-backed shim output; the root generator alone retains legacy v2/v3 input compatibility |
| `PublicSdkSurface` | v1: client/operation naming only; v2: adds stable OpenAPI component-schema → public-model naming under `models` |
| `SdkOverrides` | v1: explicit exclusions and bounded operation overrides |
| `SdkDefinition` | v2: complete public client, models and resources accepted by generation |
| `DerivationReport` | v1: outcome and machine-readable reason per relevant source operation |
| `Runtime` | Consumer-owned integration paths, with defaults; no separate schema-version field |
| `GeneratedSdk` | Generated-file map and public API inventory produced by the same lowering pass |

Canonical Bindings include Rust symbols, qualified paths, call shapes and source-operation identity, representation, statuses, discriminators and field wire names. In v4, stream semantics are stored independently from the emitted transport: `operation.stream` carries item/error/lifetime while `metadata.stream_transport` records either a named target-specific alias or a proven anonymous `impl Trait`. The shim's [JSON schema](../openapi-to-rust-bindings/rust-bindings.schema.json) and `src/contracts.rs` / `src/validation.rs` define the accepted structures; the temporary shim trusts producer metadata and proves only residual invocation facts from emitted Rust and the exact effective OpenAPI; the root generator never parses backend source. `openapi-to-rust-bindings/DEFAULT_BACKEND.json` pins the default upstream revision independently of the root generator; `COMPATIBILITY.json` tracks the same production manifest-free boundary for CI/nightly candidate checks.

### Public model identity

`PublicSdkSurface` v2 adds an explicit model-naming policy without moving API design into Bindings. The default remains v1 so existing consumers keep operation-derived model names until they deliberately opt in:

```json
{
  "schema_version": 2,
  "operations": {
    "complete_chat": ["chat.complete"]
  },
  "models": {
    "ChatCompletionRequest": {"name": "ChatRequest"},
    "UsageInfo": {"name": "Usage"}
  }
}
```

Keys under `models` are OpenAPI `components.schemas` names. Those component names are the canonical source-schema identities used by projection; backend-generated Rust symbol names are not identities. An explicit entry changes only the stable public SDK name. Unmapped named components use a deterministic public name derived from the component identity in v2. Schema v1 keeps the historical operation-derived model names.

Projection never deduplicates by field or JSON-structure equality. Two different named schemas remain different concepts unless both are explicitly mapped to the same public name, and such an explicit merge still succeeds only when their projected public contracts are compatible. Anonymous/inline schemas have compiler-local provenance but no semantic model identity; their fallback public names remain operation/path-derived and structurally identical inline schemas are not unified.

A named component can be reused by multiple operations. Representation is part of reuse safety: an owned public representation and a borrowed nested response view are not interchangeable. Borrowed views receive the deterministic `Ref` representation suffix (for example `UsageRef`) rather than being silently merged with an owned `Usage`. Incompatible attempts to claim the same public name fail with `capability.public_model_identity_collision` and identify the colliding source schemas.

A `PublicSdkSurface` entry may list multiple paths for a single source operation (for example a buffered and streaming view). Explicit `response_representations` choose among structurally supported variants; `request_overrides` and exclusions are applied only through the validated generic contract. An override of a rejected operation is an error, not a way to bypass structural validation.

The report records `derived`, `overridden`, `excluded` and `rejected` with a reason for every relevant OpenAPI operation. Check it explicitly against the consumer's expected operation inventory. A successful `derive` invocation does **not** mean every source operation was generated. Use `derive --definition-output FILE` to write the top-level `definition`
while keeping the full derivation JSON on stdout. Do not pass the entire
derivation object to `--definition`.

## CLI commands

```text
rust-sdk-generator derive --openapi FILE --bindings FILE [--surface FILE] [--overrides FILE] [--definition-output FILE]
rust-sdk-generator generate --openapi FILE --bindings FILE --definition FILE --output DIR [--runtime FILE] [--inventory FILE]
rust-sdk-generator check --openapi FILE --bindings FILE --definition FILE [--runtime FILE] [--inventory FILE]
rust-sdk-generator check-generated --openapi FILE --bindings FILE --definition FILE --output DIR [--runtime FILE]
```

`derive` prints the complete derivation and report on stdout; optional
`--definition-output` also writes its derived `SdkDefinition` to a separate
JSON file. `generate` writes generated files and prints the inventory; its optional `--inventory` writes a separate inventory file. `check` validates/compiles in memory, prints the inventory, and can write it to `--inventory`; it does not compare files on disk. `check-generated` is read-only and prints JSON arrays `missing`, `changed`, `extra` and `conflicts`; it rejects `--inventory` because that would write a file.

Exit code 0 indicates a successful command (or a clean comparison), 1 indicates a stale `check-generated` comparison and 2 indicates a CLI/input/IO/generation failure. CLI errors are JSON objects on stderr containing `code`, `message` and `path`. For safe output-directory behavior and crash recovery, read [output publication](output.md).
