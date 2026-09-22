# #157 compatibility migration: preparation and blocked cutover

Status: **preparation only**. The supported CLI/default backend, independent SDK quickstart, and ordinary/nightly production contract have **not** switched. #155 owns the supported upstream envelope and compiled HTTP matrix; #156 owns the new default CLI/backend contract. The producer-side #158 inventory is in [FORK_CAPABILITIES.json](../openapi-to-rust-bindings/FORK_CAPABILITIES.json), not an assertion of adapter support.

## Dependency inventory (current main at preparation)

| Surface | Manifest/fork dependency today | Disposition |
| --- | --- | --- |
| `.github/workflows/ci.yml` — bindings and integration | `tests/bindings.rs` and `tests/test_bindings_integration.py` read fixture manifests; `gate` requires all ordinary jobs | Preserve as explicitly labeled historical oracle. Add independent fixture-discovery/pin-helper regressions; add production envelope job **after** #155/#156. |
| `.github/workflows/ci.yml` — independent SDK | `examples/independent-sdk/quickstart.rs` takes the fork repository and baseline SHA from adapter `COMPATIBILITY.json`; default generation requests a manifest | #156 owns changing the quickstart/default. #157 must check CI runs that tested path and consumes derivation/HTTP results after #156. |
| `.github/workflows/openapi-to-rust-compat.yml` | Original daily/manual run passed moving fork `main` to a manifest-only checker, which rejects the retired producer flag | Preparation now compares two **immutable manifest-capable** historical revisions and separately resolves moving fork `main` to test its ordinary generated Rust and manifest-free Bindings against the pinned oracle. This transitional fork watch is not the future upstream/default compatibility gate; #156/#157 own that cutover. |
| `openapi-to-rust-bindings/scripts/check_backend_compat.py` | Generates `binding_manifest = true`, requires output manifest, calls positional manifest adapter; compared all OpenAPI fixture dirs regardless of purpose | Preserve historical oracle with explicit registry and fail-closed classification. The production tracker must use `--extract RAW EFFECTIVE_OPENAPI` and has to be implemented after #156. |
| `openapi-to-rust-bindings/COMPATIBILITY.json` | v2 tracker names `adriendellagaspera/openapi-to-rust@d19e5a4cba2589475dc157e50fe624576f129368`, records a manifest name | Do not repin or rewrite default metadata here. Retain last manifest-capable oracle `9fee7af67c05d896da86e41d326b067312120c53` separately for tests. #156 owns production upstream pin and schema/CLI default. |
| `openapi-to-rust-bindings/tests/fixtures/{library,menagerie,transport}` | Checked-in manifest oracle, plus `rust-bindings.json` expected historical contract; transport also has `compat.toml` / overlay | Registered legacy oracle; keep expected semantics and failure checks. Do not expose as manifest-free production evidence. |
| `openapi-to-rust-bindings/tests/fixtures/fork-semantic` | `compat.toml` requests producer manifest and request discriminator; immutable fork before removal is required | Registered legacy oracle for fresh-parity checks; do not run the fixture against unmodified upstream expecting fork-only features. |
| `openapi-to-rust-bindings/tests/fixtures/upstream-structural` and `upstream-semantic` | Ordinary Rust OpenAPI fixtures, previously also caught by broad compatibility discovery | Generic fixtures, **excluded** from manifest-only compatibility runner. Existing structural and semantic workflows still run them. |
| `openapi-to-rust-bindings/tests/fixtures/capability-v1` (on #155 branch) | Nested support-envelope fixtures with independent generation pins and configuration | Generic profile discovers nested fixtures when landed, but #157 must consume #155's actual machine-readable supported-envelope gate; this preparation does not implement or copy its HTTP tests. |
| `.github/workflows/upstream-structural-evidence.yml` and `upstream-semantic-evidence.yml` | Pin unmodified upstream `5a3487edbe27cfd4efb32dda893774e23d7fa195`; semantic workflow separately pins the manifest-era fork `9fee7af...` as parity oracle | Keep both: the upstream steps are existing **manifest-free extraction evidence**, not yet ordinary CI/compatibility coverage for every supported shape. The fork step is historical oracle only. |
| `.github/workflows/fork-manifest-removal-evidence.yml` | Pins fork before/after removal and upstream; compares ordinary raw output, old manifest and manifest-free Bindings | Retain as producer-delta/no-silent-capability-regression evidence; not a replacement for production-envelope compatibility. |
| `openapi-to-rust-bindings/tests/{bindings,semantic,structural}.rs` | `bindings.rs` tests manifest precedence, schema and sidecar behavior; `semantic.rs` / `structural.rs` test ordinary source extraction and fail-closed negative cases | Retain every legacy test; generic extraction negative tests remain required. Re-label manifest tests in reporting, not by removing code or weakening reader precedence. |
| `examples/independent-sdk/{quickstart.rs,prove.py,README.md,openapi.json,consumer}` | Native default quickstart and older Python proof request the manifest, pin the fork and assert its stream-specific call shape | Preserve current default until #156. Later update the native recipe, its tested supported envelope, source provenance and the maintained README; Python proof remains an internal historical recipe unless explicitly migrated. |
| `README.md`, `docs/{architecture,contracts,development,getting-started}.md`, `openapi-to-rust-bindings/README.md` | Describe the producer manifest, CLI manifest precedence, provenance, public onboarding and current backend | Intentionally accurate *as current behavior*, not prematurely changed. #156 updates public defaults; #157 revisits compatibility/CI instructions after actual deployment. |
| `tests/fixtures/`, `tests/oracle/`, `tests/rust_surface.rs`, root `COMPATIBILITY.json` | Backend-neutral canonical Bindings / SDK outputs; no fork manifest producer dependency | Retain their generic root generator checks and independent historical baselines without relabeling them as the upstream integration gate. |

## Fixture discovery and historical oracle

`tests/fixtures/compatibility-cases.json` explicitly registers four existing manifest-era fixtures; `scripts/compat_fixtures.py` discovers nested ordinary OpenAPI fixtures separately. Legacy registration fails if the checked-in manifest or the explicit manifest-generating configuration is absent. Future generic fixtures cannot accidentally enter the legacy manifest-only runner. The registry does **not** declare the production supported call shapes: #155 owns those.

The legacy `check_backend_compat.py` still invokes the current positional manifest loader and still requires manifest generation. Its Markdown report includes canonical source/representation drift and generated Rust diagnostics. The opt-in `--report-json` preserves the compared immutable SHAs, source and effective OpenAPI digests, exact generation configuration, per-stage raw/adapter failure status and raw generated-source diffs. Root derivation, rejected operations and cross-backend capability parity are explicitly **not run** in legacy mode rather than silently presented as passing. Machine-readable stage labels are `raw_generation`, `adapter_evidence` and `root_sdk_derivation` (the latter is reserved for the post-cutover production tracker).

For pull requests only, the historical-oracle workflow compares the tracked manifest-era baseline against the immutable last manifest-capable fork `9fee7af67c05d896da86e41d326b067312120c53`. The scheduled/manual candidate policy remains unchanged (`main` unless an explicit candidate is dispatched); do not misread a green PR historical oracle as proof that moving fork `main` or upstream production compatibility passed. The producer-removal evidence workflow continues independently to exercise the manifest-free post-removal fork.\n\nThe current fork `main` after merged fork PR #18 (`ef5632c019982de6fd8bf1a7d86a9fcd868b9c55`) rejects `binding_manifest`. The transitional CI/nightly/manual workflow now uses the last manifest-capable revision `9fee7af67c05d896da86e41d326b067312120c53` **only** for its explicitly labeled historical oracle. A second, fail-closed check still resolves the configured candidate ref (default: moving fork `main`) to an immutable SHA, generates ordinary Rust without a manifest flag, verifies source-operation coverage, extracts Bindings v3 twice with the exact fixture OpenAPI, and requires canonical parity and byte-for-byte raw-output parity against the historical oracle for the fork-semantic fixture. Any drift fails with its own stage and machine-readable report; it is not suppressed by pinning the moving candidate. This is a deliberately narrow temporary *fork* watch, **not** the #157 production upstream compatibility migration or evidence for unsupported upstream call shapes.

## Exact legacy reproduction

With this repository checked out and locked Rust tools built:

```sh
python3 -m unittest discover -s openapi-to-rust-bindings/scripts -p 'test_*.py'
cargo build --locked -p openapi-to-rust-bindings --bin openapi-to-rust-bindings

python3 openapi-to-rust-bindings/scripts/acquire_backend.py \
  --repository adriendellagaspera/openapi-to-rust \
  --ref d19e5a4cba2589475dc157e50fe624576f129368 \
  --expected-sha d19e5a4cba2589475dc157e50fe624576f129368 \
  --destination /tmp/compat-baseline
python3 openapi-to-rust-bindings/scripts/acquire_backend.py \
  --repository adriendellagaspera/openapi-to-rust \
  --ref 9fee7af67c05d896da86e41d326b067312120c53 \
  --expected-sha 9fee7af67c05d896da86e41d326b067312120c53 \
  --destination /tmp/compat-candidate
CARGO_TARGET_DIR=/tmp/compat-baseline-target cargo build --locked --release \
  --manifest-path /tmp/compat-baseline/Cargo.toml --bin openapi-to-rust
CARGO_TARGET_DIR=/tmp/compat-candidate-target cargo build --locked --release \
  --manifest-path /tmp/compat-candidate/Cargo.toml --bin openapi-to-rust
python3 openapi-to-rust-bindings/scripts/check_backend_compat.py \
  --package-root openapi-to-rust-bindings \
  --bindings-adapter target/debug/openapi-to-rust-bindings \
  --baseline-generator /tmp/compat-baseline-target/release/openapi-to-rust \
  --candidate-generator /tmp/compat-candidate-target/release/openapi-to-rust \
  --candidate-commit 9fee7af67c05d896da86e41d326b067312120c53 \
  --report /tmp/compatibility-report.md \
  --report-json /tmp/compatibility-report.json
```

The legacy comparison above remains reproducible with the two immutable producer revisions. Manual workflow dispatch with `candidate_ref=<40-character SHA>` selects the **moving-fork manifest-free watch candidate**, not the historical oracle (which remains fixed). A legacy incompatible result or candidate raw/Bindings drift fails closed; inspect the separate reports rather than updating the backend pin automatically.

## Blocked final #157 changes (do not merge into this preparation)

1. After #155: consume its **versioned supported-envelope report**, exact operation and emitted-symbol inventory, raw backend capability matrix, compiled/mock-HTTP fixture gate, deterministic Bindings and fail-closed negatives. Never infer support from declarations or require upstream to emit fork-only owned SSE/binary streams, discriminator helpers or multipart filenames that the matrix marks absent.
2. After #156: retarget the ordinary/nightly/manual tracker and `COMPATIBILITY.json` to the same upstream repository, pinned baseline, exact effective OpenAPI and `--extract` contract as the default SDK CLI. Implement the full production compatibility runner rather than silently letting the manifest-only oracle read generic fixtures; replace the temporary moving-*fork* watch with the current unmodified-upstream production boundary while retaining the immutable historical oracle explicitly.
3. In the final tracker, retain separate raw-generation, adapter evidence, and root SDK derivation/validation/compiled HTTP failure codes. Record per-fixture source/API drift, un-emitted/rejected operations, backend capability differences, canonical Bindings diffs and standalone generated-output diffs. Do not treat successfully extracted operations as proof that all advertised source operations were generated.
4. Update CI `gate` dependencies, scheduled/manual compatibility summary and maintained docs/examples only when they actually run the #156 default without a required custom manifest. Keep a separately labeled, immutable historical oracle and the no-silent-regression producer tests.
5. Leave #157 open until ordinary CI and nightly/manual production runs are green on the same manifest-free contract, with reproducible reruns and no hidden fork-manifest prerequisite.
