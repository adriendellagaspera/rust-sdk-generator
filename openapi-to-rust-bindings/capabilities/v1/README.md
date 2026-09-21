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

The matrix does not claim a native/WASM compilation proof for a stream ABI
merely because both aliases occur in generated Rust. A target not compiled by
the dedicated integration jobs remains **unverified** at that target. Do not
convert an anonymous stream into a supported shape by guessing the alias or
suppressing `extract.*` diagnostics.

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

The `--upstream-root` and `--fork-root` paths are generated fixture
directories with `core/raw`, `sse/raw` and `discriminator/raw` subfolders,
not arbitrary checkout sources. `--expected PATH` can additionally compare
the complete deterministic JSON observation with a previously reviewed report.
The `manifest-free-sdk` job is a separate, existing fork-based HTTP smoke
proof; it must not be represented as upstream-specific HTTP evidence.

No command here switches the default adapter, backend pin, init/sync,
compatibility workflows or nightly paths. Those changes belong to #156/#157.
