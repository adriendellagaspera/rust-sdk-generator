# rust-sdk-generator

Backend-neutral Rust SDK derivation and generation. The root crate derives a complete SDK definition from OpenAPI, normalized Rust Bindings, optional public-surface evidence and explicit overrides, then validates and deterministically emits idiomatic Rust source from that definition.

The repository is a Cargo workspace with two deliberately independent Rust crates:

- `rust-sdk-generator`: the backend-neutral SDK generator;
- `openapi-to-rust-bindings`: the compatibility adapter from `openapi-to-rust` output to the versioned Bindings JSON contract.

The root generator has no dependency on `openapi-to-rust` or the adapter implementation. The adapter may know `openapi-to-rust` source layout and compatibility details, but communicates with the generator only through normalized Bindings JSON.

## Integration surfaces

The canonical generator exposes equivalent library and CLI surfaces over the same implementation:

```text
rust-sdk-generator derive \
  --openapi openapi.json \
  --bindings rust-bindings.json \
  [--surface public-sdk-surface.json] \
  [--overrides sdk-overrides.json]

rust-sdk-generator generate \
  --openapi openapi.json \
  --bindings rust-bindings.json \
  --definition sdk-definition.json \
  --output src/sdk

rust-sdk-generator check \
  --openapi openapi.json \
  --bindings rust-bindings.json \
  --definition sdk-definition.json

rust-sdk-generator check-generated \
  --openapi openapi.json \
  --bindings rust-bindings.json \
  --definition sdk-definition.json \
  --output src/sdk
```

The original `check` command validates/compiles in memory and prints the public API inventory; it does **not** compare files on disk. `check-generated --output DIR` is read-only: stdout contains deterministic JSON arrays of `missing`, `changed`, `extra` and `conflicts` paths; exit code 0 means clean, 1 means stale, and 2 indicates an input/IO error (JSON diagnostic on stderr). `check-generated` does not accept `--inventory` because that would write a file.

The CLI treats a file as generator-owned only when it begins with the exact `Runtime.generated_marker` configured for that generation. It refuses to overwrite an unmarked file at a generated path. On regeneration, it removes obsolete marked files, preserves unmarked handwritten files (including `src/sdk/error.rs` in a consumer), and stages a complete output-directory copy before publishing. Output paths must refer to a dedicated SDK directory, not the repository root; symlinks/special files inside that directory are rejected rather than followed.

Publication swaps the previous directory into a uniquely named sibling backup, then moves the complete staging directory into place; on a normal second-rename failure it attempts to restore the backup. An operating-system crash between the two renames may temporarily leave the output directory absent. The next `generate` restores a single detected backup before proceeding; `check-generated` never changes the filesystem. An abandoned lock file (`.<output-name>.rust-sdk-generator.lock`) must be removed manually **only after confirming no generator is running**. Abandoned staging directories are ignored and can be removed manually. Concurrent generation targeting the same output directory is intentionally blocked by the lock.

This is not a filesystem-wide atomic transaction: non-cooperating readers can observe the brief interval between directory renames, and an optional `--inventory` path outside the output directory is written separately after the SDK has been published. Consumers requiring an uninterrupted reader view should coordinate reads with generation. Do not change the generated marker silently between releases: files carrying an older marker are deliberately considered unmanaged and cause path conflicts rather than being deleted.

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

Owned by the root Rust generator: backend-neutral OpenAPI indexing, the normalized `Bindings` consumer contract, public-surface naming/fallback, closed-world SDK derivation and derivation reporting, explicit SDK overrides, structural Rust-type reasoning, complete-definition validation, closed IR/lowering, deterministic Rust emission, canonical API inventory, runtime integration contracts, and generic fixtures/tests.

Owned by `openapi-to-rust-bindings/`: generated-source parsing, sidecar-first loading, `openapi-to-rust` source-layout assumptions, producer-side Bindings v2 validation, generic Menagerie/Library normalization fixtures, and the backend compatibility tracker. The adapter has no runtime dependency on the generator implementation. Issue #8 evolves this boundary toward generator-owned binding metadata and defines source-parser retirement.

Intentionally left in `mistralai-rs`: Mistral OpenAPI/source tracking and overlays, official SDK surface extraction, Mistral taxonomy/naming evidence, Mistral runtime integration, consumer generation/compatibility/release gates, and all Mistral-specific coverage decisions.

Consumers should pin immutable standalone commits. Language/runtime migrations in this repository do not implicitly repin downstream consumers.
