# openapi-to-rust-bindings

`openapi-to-rust-bindings` is the Rust compatibility layer between `openapi-to-rust` generator-owned metadata and the versioned `Bindings` JSON contract consumed by `rust-sdk-generator`.

The library API is deliberately small:

```rust
use openapi_to_rust_bindings::{parse_binding_manifest, read_bindings};

let bindings = read_bindings("generated")?;
let manifest = parse_binding_manifest(manifest_source)?;
# Ok::<(), openapi_to_rust_bindings::Error>(())
```

The CLI writes the same canonical value as JSON:

```text
openapi-to-rust-bindings generated > rust-bindings.json
```

`read_bindings()` consumes `binding-manifest.json` when present and fails closed if that generator-owned metadata is invalid. It also accepts a validated canonical `rust-bindings.json` sidecar when metadata has already been materialized separately. Generated `types.rs` and `client.rs` are no longer parsed as an input contract.

The crate validates canonical Bindings v3 as well as legacy v2 sidecars without depending on the root generator crate. V3 carries source-operation identity, response representation, success statuses, request discriminators, full stream ABI, and field wire names. The manifest adapter owns `openapi-to-rust` schema/layout knowledge and normalizes generated-root-relative paths into canonical `crate::generated::...` paths.

The generated-source parser was retired after the manifest equivalence matrix, standalone compatibility tracker, reviewed generator updates, and an independent downstream consumer all exercised the metadata-first path. Compatibility is now defined exclusively at the structured manifest / canonical Bindings boundary.
