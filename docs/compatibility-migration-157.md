# #157 production compatibility boundary

Status: **production cutover implemented**. Compatibility tracking now exercises the same manifest-free boundary as the default #156 quickstart: pinned unmodified `gpu-cli/openapi-to-rust`, ordinary generated Rust, the exact effective OpenAPI, `openapi-to-rust-bindings` Bindings v3 extraction, root SDK derivation/generation, and compiled HTTP consumers.

## Production source of truth

`openapi-to-rust-bindings/COMPATIBILITY.json` is the production tracker. Its baseline repository and commit must match `DEFAULT_BACKEND.json`; the checked-in supported-envelope matrix must pin the same revision. The tracker records the exact default fixture inputs:

- effective OpenAPI;
- backend generation config;
- reviewed surface and overrides;
- standalone consumer fixture;
- versioned supported-envelope matrix.

The production contract explicitly sets `producer_manifest_required=false`. A raw directory without the exact effective OpenAPI is not a valid production adapter input.

## Ordinary CI

The required `production-compatibility` CI job runs `scripts/check_production_compat.py` against the exact production pin and is part of the final `gate`. It verifies:

1. ordinary Rust generation with no `binding-manifest.json`;
2. byte-identical results from the default adapter CLI and explicit `--extract`;
3. exact source-operation coverage from the effective OpenAPI;
4. exhaustive root derivation reasons and deterministic generated inventory/facade;
5. the upstream side of `capabilities/v1/matrix.json`, including expected raw-generation and adapter-evidence gaps;
6. standalone compilation and mock-HTTP execution for the default fixture and capability-core fixture;
7. fail-closed adapter regressions through the normal bindings test suite.

The compatibility reporter classifies failures by stage: `raw_generation`, `adapter_evidence`, `root_sdk_derivation`, `compiled_http`, `supported_envelope`, or `compatibility_drift`.

## Nightly and manual candidate checks

`.github/workflows/openapi-to-rust-compat.yml` resolves both the production baseline and candidate from `gpu-cli/openapi-to-rust` to immutable SHAs. Scheduled and pull-request runs use the tracker's `candidate_ref` (currently `main`); manual dispatch may select a branch, tag, or exact commit.

The workflow builds both revisions and runs the same production reporter used by CI. Candidate evidence includes the exact effective-OpenAPI/config hashes, source identities, Bindings, exhaustive derivation, inventory, compiled HTTP proof, and versioned supported-envelope observations.

Any difference in ordinary generated Rust, canonical Bindings, derivation report, inventory, or generated SDK is reported and fails closed. The report includes bounded source diffs plus an exact immutable rerun command. A compatible candidate is not automatically repinned: the generated pin proposal still requires review.

## Supported and unsupported upstream shapes

The versioned matrix remains authoritative for the declared support envelope. Existing expected gaps are not converted into successes:

- upstream binary streaming remains a raw-generation gap when only buffered binary is emitted;
- multipart filename helpers remain a raw-generation gap when no helper method is emitted;
- request discriminator helpers remain a raw-generation gap when absent from emitted Rust;
- anonymous upstream SSE remains an adapter-evidence gap when its complete native/WASM stream ABI cannot be proven.

Fork-only evidence in `FORK_CAPABILITIES.json` and the fork side of the capability matrix remains separate from production upstream support.

## Historical manifest oracle

`openapi-to-rust-bindings/LEGACY_COMPATIBILITY.json` contains the immutable manifest-era fork tracker. `scripts/check_backend_compat.py` defaults to that explicit legacy tracker and uses `--legacy-metadata`; it is not a production fallback.

The compatibility workflow exposes the manifest-era oracle only through the manual `run_historical_oracle=true` option. Production CI, scheduled compatibility, manual upstream candidate checks, and the default quickstart do not require the fork or a producer manifest.

Historical manifest/sidecar unit and integration fixtures remain to detect regressions in the explicitly opt-in reader. The temporary moving-fork watcher used during #157 preparation has been removed.

## Reproduction

Run the required repository checks first:

```sh
python3 scripts/check_agent_contract.py
cargo fmt --all -- --check
cargo clippy -p rust-sdk-generator --all-targets --all-features -- -D warnings
cargo test -p rust-sdk-generator --all-targets --all-features
cargo clippy -p openapi-to-rust-bindings --all-targets --all-features -- -D warnings
cargo test -p openapi-to-rust-bindings --all-targets --all-features
python3 -m unittest discover -s openapi-to-rust-bindings/scripts -p 'test_*.py'
```

The public fixture path remains:

```sh
cargo run --locked --example independent-sdk-quickstart
```

For an upstream candidate, dispatch:

```sh
gh workflow run openapi-to-rust-compat.yml -f candidate_ref=<branch-tag-or-sha>
```

After resolution, rerun the exact SHA printed in the workflow summary:

```sh
gh workflow run openapi-to-rust-compat.yml -f candidate_ref=<40-character-sha>
```

The resulting `production-compatibility` artifact contains the JSON/Markdown stage report and proposed pin update.

## Boundary after #157

The root generator remains backend-neutral and never parses `openapi-to-rust` source. Backend-specific structural and semantic interpretation stays in `openapi-to-rust-bindings`. Historical manifests are explicit oracle inputs only. Consumer source-update policy and the future generic own-API `init`/`sync` workflow remain outside this nightly orchestration.
