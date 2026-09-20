# openapi-to-rust-bindings

Adapter from the `openapi-to-rust` backend's structured `binding-manifest.json` to the backend-neutral Bindings JSON contract consumed by `rust-sdk-generator`. This crate has no runtime dependency on the root generator.

## Input precedence

`read_bindings(directory)` reads `binding-manifest.json` when present and converts it to canonical Bindings v3. An invalid manifest is an error, even if a sidecar also exists. If there is no manifest, a validated `rust-bindings.json` sidecar is accepted (v2 or v3). When neither exists, loading fails. Generated `types.rs` and `client.rs` are not parsed or treated as a fallback.

The manifest parser validates the backend's structured schema and layout, normalizes symbol paths into `crate::generated::...` paths and preserves the metadata required by generic derivation: source-operation identity, response representation, success statuses, request discriminators, stream ABI and wire field names. The root generator, not this adapter, chooses the public SDK surface and applies consumer policy.

## Usage

```sh
cargo run --quiet -p openapi-to-rust-bindings -- path/to/raw-output > rust-bindings.json
```

The library exports `read_bindings(path)`, `parse_binding_manifest(&str)`, `Bindings::from_value(Value)` and `Bindings::as_value()`. The CLI writes the canonical value to stdout as pretty JSON. `MANIFEST_NAME` and `SIDECAR_NAME` expose the accepted file names.

The authoritative producer pin and manifest contract are recorded in [COMPATIBILITY.json](COMPATIBILITY.json). The independent backend compatibility workflow compares a pinned baseline and a candidate using canonical Bindings, not source-text parsing; a separately pinned standalone SDK proof is described in [the example README](../examples/independent-sdk/README.md).

See [architecture](../docs/architecture.md) and [contracts](../docs/contracts.md) for the boundary with the root generator.
