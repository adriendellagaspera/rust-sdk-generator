# Getting started with a standalone Rust SDK

## Run the independent example

Requirements: Rust 1.88+ with Cargo, Git, and network access on the first run.
From this repository's root:

```sh
cargo run --locked --example independent-sdk-quickstart
```

This **native Rust** example checks out the exact raw-backend commit from
[`COMPATIBILITY.json`](../openapi-to-rust-bindings/COMPATIBILITY.json),
builds the raw backend, adapter and generator from locked Cargo dependencies,
then runs the entire generation pipeline twice and compares its output
byte-for-byte. Finally it compiles a standalone consumer crate and runs its
local HTTP mock tests. The same command runs in the `independent-sdk` CI job;
no Python interpreter or existing consumer SDK is required.

The command prints a persistent working directory. To choose a new or empty
directory yourself, run:

```sh
cargo run --locked --example independent-sdk-quickstart -- \
  --work-dir /tmp/rust-sdk-getting-started
```

Inspect these artifacts inside the printed directory:

| Path | Role |
| --- | --- |
| `_backend/`, `_backend-target/`, `_tools-target/` | Verified pinned source and locally compiled tools |
| `first/openapi-to-rust.toml`, `first/raw/` | Backend configuration, raw Rust client and its manifest |
| `first/rust-bindings.json` | Adapter-produced canonical Bindings v3 |
| `first/derivation.json`, `first/definition.json` | Exhaustive operation report and derived SDK definition |
| `first/sdk/`, `first/inventory.json` | Public facade sources and API inventory |
| `second/` | Independent generation used to verify determinism |
| `consumer/` | Standalone generated crate, handwritten runtime and HTTP tests |

The example is **turnkey for the notebook fixture**, not for an arbitrary new
API. A reusable Rust-native `init`/`sync` workflow for user-supplied OpenAPI
is tracked in [#146](https://github.com/adriendellagaspera/rust-sdk-generator/issues/146).
The currently pinned raw backend emits a custom manifest; eliminating that
requirement in favor of an unmodified upstream backend is tracked in
[#147](https://github.com/adriendellagaspera/rust-sdk-generator/issues/147).
Neither feature is implicitly provided by this quickstart.

## Follow the inputs and output

The raw backend generates Rust models and the HTTP client **from OpenAPI JSON**.
Its backend-owned manifest records emitted symbols, operation identities and
transport evidence. The `openapi-to-rust-bindings` adapter normalizes that
evidence into canonical `Bindings` v3; the backend-neutral root generator
does not parse the backend's Rust source or invoke its CLI.

`PublicSdkSurface` optionally supplies reviewed client/resource/method naming
evidence. `SdkOverrides` supplies explicit consumer decisions, such as
excluding an operation with a reason or choosing a supported response
representation. Neither can override unproven raw HTTP/type behavior.
The example's `surface.json` exposes both buffered and streaming binary
methods for one source operation, with the selection specified in
`overrides.json`.

The Rust `derive` command returns `Derivation`: a validated
`SdkDefinition` and an exhaustive `DerivationReport`. Every relevant
OpenAPI operation has a `derived`, `overridden`, `excluded`, or `rejected`
status and an explicit `reason.code`. Inspect
`first/derivation.json` (especially `report.operations`) before deciding
whether a generated SDK has the coverage your consumer needs. A successful
`derive` run alone is **not** a guarantee of complete operation coverage.
An unsupported shape must remain rejected rather than being silently
discarded or accommodated by fabricated metadata.

`generate` validates and lowers the complete definition to a deterministic
public facade. `Runtime` specifies how this facade links to the consumer's
raw client and error exports; the example supplies a minimal handwritten
runtime under `consumer/src/sdk/error.rs`. The public facade is **not** the
raw backend client, and the example runtime is not a production runtime.
Authentication, retries, crate identity and publishing policy remain the SDK
maintainer's responsibility. See [architecture](architecture.md) and
[versioned contracts and CLI](contracts.md) for the precise boundary.

## Adapt the workflow to your own OpenAPI document

The example is a reference integration, not a generic initializer. Until
[#146](https://github.com/adriendellagaspera/rust-sdk-generator/issues/146),
you must supply a backend configuration for your OpenAPI **JSON** file, run
the selected pinned raw backend, integrate its Rust sources/dependencies and
provide a handwritten runtime in your consumer crate. The generated
`first/openapi-to-rust.toml` shows the current backend options; this
particular backend requires `binding_manifest = true` until #147/#151
complete the manifest-free adapter migration.

Normalize the generated raw directory with
`openapi-to-rust-bindings <raw-output-dir> > rust-bindings.json`.
Then derive the SDK without supplying naming evidence or overrides until a
reviewed decision is necessary:

```sh
rust-sdk-generator derive \
  --openapi openapi.json --bindings rust-bindings.json \
  --definition-output sdk-definition.json > derivation.json

rust-sdk-generator generate \
  --openapi openapi.json --bindings rust-bindings.json \
  --definition sdk-definition.json --output src/sdk

rust-sdk-generator check-generated \
  --openapi openapi.json --bindings rust-bindings.json \
  --definition sdk-definition.json --output src/sdk
```

The native `--definition-output` option avoids a Python/JSON extraction
script. Read `derivation.json` for all operation outcomes before running
`generate`. `check-generated` reads the existing output without writing it:
exit code 0 means clean, 1 reports missing/changed/extra/conflicting generated
files as JSON, and 2 indicates an input or IO error. The older `check`
command validates in memory and prints the API inventory, **not** a disk diff.

Regeneration owns only files carrying the configured
`Runtime.generated_marker`. It preserves handwritten runtime files and
rejects path conflicts with unmanaged files; do not silently change the
marker. Read [output safety and recovery](output.md) before pointing the CLI
at a consumer's source directory.

The independent fixture proves JSON requests/responses, path/query
parameters, optional/nullable fields, documented errors, empty DELETE, and
buffered/streamed binary export. It does **not** prove universal OpenAPI 3.x,
all union/map/multipart/event-stream schema shapes, arbitrary authorization
schemes, or a reusable production runtime. Refer to the report for your
specific API; a missing structural proof is a documented limitation, not an
invitation to relax validation.
