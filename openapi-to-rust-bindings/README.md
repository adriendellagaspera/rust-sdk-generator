# openapi-to-rust-bindings

`openapi-to-rust-bindings` is the compatibility layer between `openapi-to-rust` output and `rust-sdk-generator`.

It exposes a deliberately small functional API:

```python
from openapi_to_rust_bindings import parse_bindings, read_bindings

bindings = read_bindings(generated_dir)
# or, for generated-source compatibility
bindings = parse_bindings(types_source, client_source)
```

`read_bindings()` prefers a versioned `rust-bindings.json` sidecar when present and fails closed if that sidecar is invalid. For older generator output without a sidecar, it falls back to normalizing `types.rs` and `client.rs`.

Both functions return the public `Bindings` type from `rust-sdk-generator`. The generated-source fallback owns all knowledge of `openapi-to-rust` source layout and conventions; the sidecar path needs only the versioned `Bindings` contract. The package does not import compiler internals.

The package is validated against the exact `openapi-to-rust` backend revision recorded in `COMPATIBILITY.json` and against an exact `rust-sdk-generator` commit.
