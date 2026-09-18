# openapi-to-rust-bindings

`openapi-to-rust-bindings` is the Rust compatibility layer between `openapi-to-rust` output and the versioned `Bindings` JSON contract consumed by `rust-sdk-generator`.

The library API is deliberately small:

```rust
use openapi_to_rust_bindings::{parse_binding_manifest, parse_bindings, read_bindings};

let bindings = read_bindings("generated")?;
let manifest = parse_binding_manifest(manifest_source)?;
let fallback = parse_bindings(types_source, client_source)?;
# Ok::<(), openapi_to_rust_bindings::Error>(())
```

The CLI writes the same canonical value as JSON:

```text
openapi-to-rust-bindings generated > rust-bindings.json
```

`read_bindings()` first consumes `binding-manifest.json` when present and fails closed if that generator-owned metadata is invalid. During the bounded migration it next accepts a canonical `rust-bindings.json` sidecar, then falls back to normalizing `types.rs` and `client.rs` only when neither metadata file exists. A broken manifest never falls through to source parsing.

The crate validates canonical Bindings v3 as well as legacy v2 without depending on the root generator crate. V3 carries source-operation identity, response representation, success statuses, request discriminators, full stream ABI, and field wire names. The manifest adapter owns `openapi-to-rust` schema/layout knowledge and normalizes generated-root-relative paths into canonical `crate::generated::...` paths.

The generated-source parser is migration-only. Remove it once all four proofs hold: generic fixtures use the manifest path; the historical common-contract equivalence matrix is green; a real Mistral consumer completes its full gate from the manifest path; and at least one real `openapi-to-rust` upgrade passes the standalone canonical-Bindings compatibility tracker. Until then, parser fallback remains supported only when generator-owned metadata is absent.
