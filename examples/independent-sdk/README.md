# Independent SDK proof (#134)

This example uses a deliberately independent notebook API fixture. The OpenAPI document declares JSON creation and lookup, path and
optional query parameters, an optional nullable field, an empty DELETE,
documented JSON errors, and binary export. The pinned raw backend also emits
buffered and byte-streaming binary methods; the public surface explicitly
selects both. No downstream SDK definition, runtime, or build scripts are used;
this proof does not implement the separate Progenitor adapter.

## Reproduce (Python 3.11+, Rust 1.88+, Git)

Run from the root of this repository. The backend revision is read from the
same immutable compatibility pin used by the adapter's compatibility check.

```sh
set -euo pipefail
BACKEND_COMMIT="$(python3 -c 'import json; print(json.load(open("openapi-to-rust-bindings/COMPATIBILITY.json"))["backend"]["baseline"]["commit"])')"
WORK="$(mktemp -d)"
git init "$WORK/backend"
git -C "$WORK/backend" remote add origin https://github.com/adriendellagaspera/openapi-to-rust.git
git -C "$WORK/backend" fetch --depth=1 origin "$BACKEND_COMMIT"
git -C "$WORK/backend" checkout --detach FETCH_HEAD
test "$(git -C "$WORK/backend" rev-parse HEAD)" = "$BACKEND_COMMIT"

CARGO_TARGET_DIR="$WORK/backend-target" cargo build --locked --release \
  --manifest-path "$WORK/backend/Cargo.toml" --bin openapi-to-rust
cargo build --locked -p openapi-to-rust-bindings --bin openapi-to-rust-bindings
cargo build --locked -p rust-sdk-generator --bin rust-sdk-generator

python3 examples/independent-sdk/prove.py \
  --backend "$WORK/backend-target/release/openapi-to-rust" \
  --adapter target/debug/openapi-to-rust-bindings \
  --generator target/debug/rust-sdk-generator \
  --work-dir "$WORK/proof"
```

The script makes two independent raw generations from `openapi.json`. On
each pass it requires the raw backend's own `binding-manifest.json`, runs the
bindings adapter to obtain canonical Bindings v3, calls `derive` with
`surface.json` and `overrides.json`, and passes the actual derived
definition to `generate` and `check`. It checks exact OpenAPI operation
coverage, each report outcome and reason, the projected operations, response
transport selection, output file inventory and byte-for-byte deterministic
outputs. It does not suppress or silently drop unsupported operations: if
any of this fixture's four operations becomes rejected, the proof fails with
its reported reason rather than relaxing structural checks. The existing
generator regression fixtures separately exercise explicit rejection for
unproven structures.

The script then constructs a disposable standalone Cargo crate from
`consumer/`, the raw backend's Rust sources and required dependency
fragment, and the emitted SDK facade. The consumer's only hand-written
runtime is `consumer/src/sdk/error.rs`. Its local TCP mock tests HTTP
serialization, deserialization, empty/optional/nullable fields, API error
status/body and buffered/streaming binary responses. The crate imports
neither generator nor adapter nor any downstream SDK code. It uses a fresh Cargo
dependency resolution for the disposable consumer; the raw backend and
both generation crates are built from their locked dependency graphs.

Inspect `$WORK/proof/first/{raw,rust-bindings.json,derivation.json,definition.json,inventory.json,sdk}`
and `$WORK/proof/consumer/src/{generated,sdk}`. The expected facade
file inventory is `facade_types.rs`, `mod.rs`, `notes.rs`. The second
pass is available at `$WORK/proof/second`. CI runs this as its own
`independent-sdk` job in parallel with the existing jobs and includes it
in the stable final `gate`.

## Failure ownership

| Stage prefix | Responsible boundary | Inspect |
| --- | --- | --- |
| `[raw backend]` | Pinned `openapi-to-rust` CLI, generated Rust or generator-owned manifest | `first/raw/` and `first/openapi-to-rust.toml` |
| `[bindings adapter]` | Manifest to canonical Bindings v3 normalization | `first/raw/binding-manifest.json`, `first/rust-bindings.json` |
| `[generator derive]`, `[generator report]`, `[generator generate]`, `[generator files]`, `[generator check]`, `[generator inventory]` | Backend-neutral derivation, structural proof, emission and inventory | `first/derivation.json`, `first/definition.json`, `first/sdk/` |
| `[determinism ...]` | Repeatability across otherwise identical complete passes | Compare `first/` and `second/` |
| `[consumer compile and HTTP tests]` | Standalone crate integration, raw HTTP transport, minimal consumer runtime or test assertions | `consumer/Cargo.toml`, `consumer/src/`, `consumer/tests/http.rs` |

This example is a focused integration contract, not full OpenAPI or
streaming/SSE coverage, an independently pinned consumer lockfile, or a
publicly supported runtime. A genuine unsupported operation should remain
`rejected` in the exhaustive report; extending structural support belongs
to a separate generic implementation and regression test.
