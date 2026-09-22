# Independent SDK proof (#134, #138)

This independent notebook API fixture covers JSON creation and lookup,
path/query parameters, optional and nullable fields, empty DELETE, documented
JSON errors, and buffered binary export (the pinned upstream does not emit fork-only live binary streaming). It uses no downstream
SDK definition, runtime, or build scripts.

## Reproduce with Rust

Requirements: Rust 1.88+ with Cargo, Git, and first-run network access.
From this repository's root:

```sh
cargo run --locked --example independent-sdk-quickstart
```

To retain artifacts at a chosen **new or empty** location:

```sh
cargo run --locked --example independent-sdk-quickstart -- \
  --work-dir /tmp/notebook-sdk-proof
```

The Rust runner in `quickstart.rs` fetches and verifies the immutable
raw-backend revision in `openapi-to-rust-bindings/DEFAULT_BACKEND.json`, builds
the backend/adapter/generator, runs two independent full generations, and
compares the raw sources, Bindings, derivation report, facade and inventory.
It also checks generated-file freshness without writing output. Finally it
assembles the standalone Cargo consumer from `consumer/`, the raw backend's
Rust sources and dependency fragment, and the generated public facade.
Its minimal handwritten runtime is `consumer/src/sdk/error.rs`.

The consumer's local TCP mock tests exercise actual HTTP request
serialization, response deserialization, error statuses/bodies and buffered
and buffered binary responses. It imports neither the generator, adapter nor
any other SDK. The same Cargo command runs in the CI `independent-sdk` job
and is required by the final `gate`; Python is not part of this user path.

Inspect `<work-dir>/first/{raw,rust-bindings.json,derivation.json,definition.json,inventory.json,sdk}`,
`<work-dir>/second/` and `<work-dir>/consumer/src/{generated,sdk}`.
The expected public facade consists of `facade_types.rs`, `mod.rs` and
`notes.rs`. The [getting-started guide](../../docs/getting-started.md)
explains each input contract, how to inspect rejected operations, and how to
adapt the pipeline to another OpenAPI document.

## Failure ownership

| Stage prefix | Responsible boundary | Inspect |
| --- | --- | --- |
| `[backend checkout]`, `[backend pin]`, `[backend build]`, `[raw backend]` | Pinned unmodified upstream and its ordinary generated Rust | `_backend/`, `first/raw/`, `first/openapi-to-rust.toml`, `first/openapi.json` |
| `[bindings adapter]` | Generated Rust + exact effective OpenAPI to Bindings v3 | `first/raw/`, `first/openapi.json`, `first/rust-bindings.json` |
| `[generator derive]`, `[generator report]`, `[generator generate]`, `[generator freshness]` | Backend-neutral derivation, structural evidence, emission and freshness | `first/derivation.json`, `first/definition.json`, `first/sdk/` |
| `[two independent complete generations]` | Repeatability across two complete passes | Compare `first/` and `second/` |
| `[consumer compile and HTTP tests]` | Standalone crate integration, raw HTTP transport, minimal runtime and tests | `consumer/Cargo.toml`, `consumer/src/`, `consumer/tests/http.rs` |

This fixture is not a universal OpenAPI coverage proof, an independently
pinned consumer lockfile, a general-purpose runtime, or a replacement for the
future generic `init`/`sync` workflow. Genuine unsupported operations must
remain explicitly rejected.

The first-run path rejects `binding-manifest.json` in raw output and compares
both complete generation passes byte-for-byte. The
[versioned capability matrix](../../openapi-to-rust-bindings/capabilities/v1/README.md)
records supported and unsupported upstream emitted variants. Fork-only streams,
filename helpers and request discriminators are not silently advertised by
this default recipe. This fixture does not implement #146's own-API `init`/`sync`.
