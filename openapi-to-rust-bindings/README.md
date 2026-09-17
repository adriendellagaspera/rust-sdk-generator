# openapi-to-rust-bindings

`openapi-to-rust-bindings` is the compatibility layer between `openapi-to-rust` output and the versioned `Bindings` JSON contract consumed by `rust-sdk-generator`.

It exposes a deliberately small functional API:

```python
from openapi_to_rust_bindings import parse_bindings, read_bindings

bindings = read_bindings(generated_dir)
# or, for generated-source compatibility
bindings = parse_bindings(types_source, client_source)

sidecar = bindings.to_dict()
```

`read_bindings()` prefers a versioned `rust-bindings.json` sidecar when present and fails closed if that sidecar is invalid. For older generator output without a sidecar, it falls back to normalizing `types.rs` and `client.rs`.

Both functions return the adapter-owned immutable `Bindings` value. The package validates the same version-2 JSON shape that the Rust generator consumes, but it does not import or depend on the generator runtime. The generated-source fallback owns all knowledge of `openapi-to-rust` source layout and conventions; the sidecar path needs only the versioned contract.

The exact `openapi-to-rust` backend revision validated by the compatibility fixtures is recorded in `COMPATIBILITY.json`. Compatibility with generator consumers is expressed through `bindings_schema_version`, not a runtime package dependency or compiler commit pin.
