# openapi-to-rust-bindings

`openapi-to-rust-bindings` is the Rust compatibility layer between `openapi-to-rust` output and the versioned `Bindings` JSON contract consumed by `rust-sdk-generator`.

The library API is deliberately small:

```rust
use openapi_to_rust_bindings::{parse_bindings, read_bindings};

let bindings = read_bindings("generated")?;
let fallback = parse_bindings(types_source, client_source)?;
# Ok::<(), openapi_to_rust_bindings::Error>(())
```

The CLI writes the same canonical value as JSON:

```text
openapi-to-rust-bindings generated > rust-bindings.json
```

`read_bindings()` prefers a versioned `rust-bindings.json` sidecar when present and fails closed if that sidecar is invalid. For older generator output without a sidecar, it falls back to normalizing `types.rs` and `client.rs`.

The crate validates the same version-2 JSON shape that the root Rust generator consumes, but it does not depend on the generator crate. The generated-source fallback owns all knowledge of `openapi-to-rust` source layout and conventions; the sidecar path needs only the versioned contract.

The exact `openapi-to-rust` backend revision validated by the compatibility fixtures remains recorded in `COMPATIBILITY.json`. The Rust migration does not repin that backend. Issue #8 will move the steady-state boundary to generator-owned binding metadata and retire this source parser once equivalence has been proved.
