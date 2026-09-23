# Local-first own-OpenAPI SDK adoption

This workflow is distinct from the fixed notebook quickstart. It accepts a local OpenAPI 3.1 JSON document within the pinned upstream's *proven* Rust envelope, not universal OpenAPI/YAML/URL input. Every relevant operation must have a source operationId, manifest-free Bindings v3 evidence and a supported structural derivation; rejected operations fail the run.

## Clean-machine commands

Requirements: Rust 1.88+ (Cargo), Git, a working native toolchain, and first-run network access to GitHub/crates.io. Start with a fixed generator checkout:

```sh
git clone https://github.com/adriendellagaspera/rust-sdk-generator.git
cd rust-sdk-generator
cargo run --locked --bin sdk-adopter -- init \
  --openapi /absolute/path/to/your-openapi.json \
  --output /absolute/path/to/your-new-sdk --name your-new-sdk
cargo test --locked --manifest-path /absolute/path/to/your-new-sdk/Cargo.toml --all-targets
```

The Cargo init invocation acquires/builds the exact pinned unmodified upstream backend, builds the Rust adapter/root CLI, derives from the user's own JSON and canonical Bindings v3, writes the standalone crate and compiles it before publication. No Python or hand-authored full SdkDefinition is required. Init refuses any pre-existing destination. The original JSON is *copied* into the crate as openapi.json; edit that checked-in copy for later sync. The crate's Cargo.toml is consumer-owned, with publish=false and no invented license, auth scheme or production release policy.

## Inspect, accept and sync

```sh
# After modifying your-new-sdk/openapi.json:
cargo run --locked --bin sdk-adopter -- sync --crate /absolute/path/to/your-new-sdk --check
cargo run --locked --bin sdk-adopter -- sync --crate /absolute/path/to/your-new-sdk --accept-coverage
cargo test --locked --manifest-path /absolute/path/to/your-new-sdk/Cargo.toml --all-targets
```

--check leaves consumer sources unchanged and reports each operation's derivation status/reason, the public API inventory, the root read-only generated-source freshness diff (missing/changed/extra/conflicting paths), previous/candidate generated Rust source and digests for each affected path, and source/raw digests. A normal sync refuses changed operation coverage or reason codes; after reviewing the report, --accept-coverage records the change. When the effective source has not changed, any raw-source digest drift or missing/changed/extra marked public facade is reported and rejected as `sync.nondeterminism`, including under `--check`; restoring the accepted bytes or an explicit recipe migration is required. It **never** permits a rejected operation or evidence conflict. A sync without coverage changes needs no flag. The root generator validates, generates and publishes the public facade through its existing marker, lock, staging and recovery semantics; raw source has additional SHA-256 ownership verification. A temporary standalone crate must compile before updating an existing consumer. These two output trees are not one atomic multi-directory filesystem transaction; coordinate builds during sync.

On a previously bootstrapped machine, append --offline to init or sync. The cache defaults to $HOME/.cache/rust-sdk-adopter/v1; override with RUST_SDK_ADOPTER_CACHE. Offline mode never fetches a missing backend and uses Cargo --offline; a missing checkout, compiled binary or Cargo registry artifact fails explicitly. Pin the generator checkout itself independently.

## Versioned recipe, deterministic inputs and file ownership

sdkgen.lock.json v1 records relative source path and SHA-256, selected backend ID/repository/immutable 40-character SHA, exact v1 backend options (module name sdk, localhost initial URL, retries disabled, pruning disabled), generated layout, adapter identifier, Bindings version 3, generator commit/version, SdkDefinition v2 and DerivationReport v1 contracts, reviewed runtime template version 1, crate identity and output paths, the backend's exact dependency-fragment SHA-256, optional PublicSdkSurface v1/SdkOverrides v1 filenames and hashes, accepted operation coverage, individual owned raw-file hashes and accepted derivation/inventory SHA-256. Sync checks that the previous accepted reports exist and match their locked digests before doing any work; restoring or explicitly migrating stale evidence is a consumer decision. Sync does not adopt a newer generator/backend revision or a different backend; migrations need a separately reviewed recipe/driver/template change. Commit the recipe alongside source and the generated audit.

| Files | Ownership |
| --- | --- |
| openapi.json, Cargo.toml, Cargo.lock | Consumer; edit the copied source and review dependencies/crate identity. |
| src/lib.rs, src/sdk/error.rs, tests | Consumer; the minimal error/runtime adapter is a reviewed starting point, not a production taxonomy. |
| src/generated/*.rs | Backend-driver owned by exact digest; unrecognized, missing or edited files cause sync to fail. |
| src/sdk/*.rs other than error.rs | Root-generator owned iff carrying the current marker; handwritten files are preserved, path collisions fail. |
| sdkgen.lock.json, .sdkgen/derivation.json, .sdkgen/inventory.json | Generation recipe, exhaustive accepted report and public API inventory; review before release. |

The starter integrates the exact backend REQUIRED_DEPS.toml fragment, adding only missing facade dependencies bytes = "1" and futures-util = "0.3" and an explicit test-only Tokio dependency. The consumer Cargo.lock fixes transitive versions. The generator and adapter are build-time tools only, never runtime dependencies of the SDK crate. A changed backend dependency fragment is a migration, not permission to overwrite the consumer Cargo.toml.

On init, --surface FILE and --overrides FILE copy explicit naming/override evidence without allowing it to invent transport semantics. Their hashes are locked; changing them later requires a reviewed new init/recipe migration, not an implicit sync.

## Diagnostics, envelope, HTTP and backend replaceability

Failures emit JSON stderr with stage and message. Stage families include source.json/source.openapi, backend.checkout/backend.pin/backend.build/backend.offline_cache, raw.generate/raw.layout, adapter.extract/bindings.v3, derive.contract/derive.unsupported/sync.coverage_drift, sdk.diff/sdk.conflicts/sdk.publish/raw.ownership, crate.compile, and the consumer-owned HTTP test stage. The independently maintained capability matrix documents exercised upstream JSON/text/empty/buffered-binary operations, bounded path/query/header parameters and HTTP errors. Unproven anonymous SSE stream ABI and fork-only raw variants are rejected. Check the per-API exhaustive report rather than treating green derivation as universal OpenAPI coverage.

The public init/sync commands consume a narrow driver output (ordinary Rust, exact dependencies and canonical Bindings v3). The *single implemented* upstream driver owns acquisition, immutable pin, TOML/CLI invocation, raw layout and manifest-free adapter selection. The root generator remains backend-neutral and owns reconciliation/derivation/generation. A future OpenAPI Generator driver would require its own demonstrated OpenAPI 3.1/Rust envelope, adapter, reviewed runtime integration and explicit migration; no second backend is implemented today. The minimal starter's raw ApiOpError mapping and public Client::new(api_key) are versioned upstream integration assumptions, not universal authentication or error handling.

The independent field-station fixture in bash scripts/prove_adopter.sh exercises clean init -> standalone compile/mock HTTP -> modified OpenAPI -> read-only report/diff -> reviewed sync -> compile/mock HTTP -> byte-identical repeat sync plus fail-closed negatives. The own-api-sdk CI job is mandatory alongside the existing notebook and compatibility proofs. The initial green independent own-API run on an Ubuntu 24.04 GitHub Actions runner measured **164.73 seconds wall time** for the one Cargo init command (first backend checkout/build, tool bootstrap, standalone compilation included); this is a runner-specific measurement, not a universal SLA. The basic journey uses **one Cargo init command after cloning** and zero manually authored SDK definitions or runtime lines to compile; consumer authentication, network/retry policy, license, published identity, production taxonomy, API compatibility and release checks all require explicit maintainer review. Do not present the notebook timing as an own-API benchmark.
