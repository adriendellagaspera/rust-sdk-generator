# Manifest-free capability matrix v1

The [versioned expectations](matrix.json) and the `capability-matrix` Rust
example define one backend-neutral compatibility gate. The input is the **exact
effective OpenAPI** used for generation. Ordinary upstream output must not
contain `binding-manifest.json`; a fresh fork manifest is used only as an
independent oracle. Both backend revisions are immutable:

| Backend | Revision |
| --- | --- |
| `gpu-cli/openapi-to-rust` | `5a3487edbe27cfd4efb32dda893774e23d7fa195` |
| `adriendellagaspera/openapi-to-rust` | `9fee7af67c05d896da86e41d326b067312120c53` |

`matrix.json` records the declared response/transport selectors to be
exercised and their **expected evidence outcome**, not a claim derived from
OpenAPI alone. The example reads the emitted `client.rs` and `types.rs`,
inspects actual method signatures/bodies through the adapter, and fails if
extraction results, canonical Bindings v3 or the fork oracle diverge. The report
contains OpenAPI declarations, raw method signatures, adapter evidence and
diagnostics, and full canonical Bindings per scenario. The report is generated
on every run and is deterministic; the compact expectations are checked in for
review.

## Evidence ownership

The upstream core fixture exercises JSON, text, empty and buffered-binary
responses; path/query/header parameters; JSON and multipart request bodies;
optional, required-nullable and optional-nullable request fields; selected 2xx
statuses; and non-2xx error handling. CI additionally compiles the
`generated` and `sdk` modules in a separate Cargo workspace and calls the
*actual upstream-generated methods* against a local mock HTTP server.

The OpenAPI binary response does not, by itself, demonstrate binary-stream
generation. The fork's additional streamed method is recorded separately from
upstream's buffered method. Likewise, an OpenAPI multipart body is not evidence
of a filename-helper method, and a request field is not evidence of a
transport-specific discriminator assignment.

The pinned upstream emits an SSE method with an anonymous `impl Stream`
return type, not the complete owned native/WASM alias required by the current
Bindings v3 extraction contract. This is classified as an **adapter-evidence
gap** (`extract.stream_abi_unproven`), not proof that upstream lacks SSE
transport entirely. Binary-stream, multipart-filename-helper and
transport-specific request-discriminator variants are recorded as
**raw-generation gaps** when the corresponding methods are absent. The
fork-provided variants remain separately compared with a freshly generated
manifest oracle. Actual upstream raw-generation remediation is owned by #158.

The stream target-proof entries in `matrix.json` are **observed compile
outcomes**, separate from OpenAPI declarations, raw emission and adapter
proof. The pinned fork's binary-stream, SSE and transport-discriminator
variants are compiled in independent native consumers and exercised through
real generated HTTP methods (including a 404/401 error, streamed bytes,
multipart filename and serialized discriminator values). The same generated
consumers' raw clients and facades also pass `cargo check --lib --target
wasm32-unknown-unknown` in CI. **WASM execution or actual HTTP behavior is
not asserted**; only cross-target compilation is verified. The unmodified
upstream's anonymous SSE ABI remains rejected before cross-target compilation,
and its missing binary stream cannot be promoted to supported simply because
the fork compiled. Never suppress `extract.*` diagnostics.

## Reproduction

From the repository root, first run the canonical local checks in
[AGENTS.md](../../../AGENTS.md). Then run the
[Upstream semantic evidence workflow](../../../.github/workflows/upstream-semantic-evidence.yml)
on the PR. Its `upstream-only-semantic` job uses independent detached checkouts
at the SHAs above, builds both binaries with `cargo build --locked`, generates
each `capability-v1` fixture from the recorded TOML config, and verifies the
upstream raw → manifest-free adapter → canonical Bindings v3 → derive/generate
→ independent consumer compilation and local mock HTTP behavior. It runs the
Rust capability example with:

```sh
cargo run --locked -p openapi-to-rust-bindings --example capability-matrix -- \
  --matrix openapi-to-rust-bindings/capabilities/v1/matrix.json \
  --upstream-root "$RUNNER_TEMP/capability-upstream" \
  --fork-root "$RUNNER_TEMP/capability-fork" \
  --output "$RUNNER_TEMP/capability-report.json"
```

Successful `upstream-only-semantic` jobs also publish the deterministic
`capability-report.json` as the `manifest-free-capability-observation-v1`
GitHub Actions artifact. This is the per-source observation for reviewing
future compatibility diffs; `matrix.json` remains the concise versioned
expectation file, not a substitute for observing emitted Rust.

The `--upstream-root` and `--fork-root` paths are generated fixture
directories with `core/raw`, `sse/raw` and `discriminator/raw` subfolders,
not arbitrary checkout sources. `--expected PATH` can additionally compare
the complete deterministic JSON observation with a previously reviewed report.
The same workflow also separately proves **fork-only** observed shapes.
For each freshly generated `core`, `sse` and `discriminator` fork fixture,
it extracts canonical Bindings v3 without reading the producer manifest,
derives/generates a public facade, compiles a separate consumer and runs its
actual generated calls against TCP mock HTTP fixtures (native target). It
then installs the wasm32 Rust standard-library target and runs
`cargo check --manifest-path "$RUNNER_TEMP/fork-$scenario-consumer/Cargo.toml"
--target wasm32-unknown-unknown --lib` for each scenario. Successful native
and WASM *compilation* is distinct from the pinned producer-manifest oracle
comparison and from the unsupported upstream stream ABI.

The `manifest-free-sdk` job is the separate native #156 proof of the pinned unmodified-upstream default fixture. Production CI/nightly compatibility additionally runs the versioned upstream envelope and its independent capability-core HTTP fixture.

The default adapter/backend and compatibility workflow now share the same upstream pin. The fork half of this evidence remains an explicit oracle only and does not participate in the production compatibility boundary.
