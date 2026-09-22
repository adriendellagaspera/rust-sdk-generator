# Architecture and ownership

The workspace separates backend-dependent Rust metadata production from SDK decisions and generated source. The root crate must remain useful with any backend producing an equivalent validated Bindings JSON contract.

```text
independently supplied OpenAPI ------+----------------------------+
                                    |                            |
pinned upstream -> ordinary Rust + exact effective OpenAPI     |
                          |                                     |
           openapi-to-rust-bindings                              |
                          |                                     |
                  canonical Bindings v3 ------------------------+
                          |                                     |
         optional public surface / explicit overrides          |
                          +---------------------+---------------+
                                                |
                         rust-sdk-generator::derive()
                                                |
                         { definition, exhaustive report }
                                                |
                        inspect exclusions and rejections
                                                |
                 rust-sdk-generator::generate() / check
                                                |
                     generated Rust files + API inventory
                                                |
                 consumer-owned raw backend code + runtime
                                                |
                      independently compiled SDK
```

The raw backend owns emitted Rust, operation signatures and generation configuration. The adapter owns generated-Rust inspection, reconciliation against effective OpenAPI and canonical Bindings validation; it never chooses public resource names or request behavior. The root generator owns OpenAPI indexing, source-operation reconciliation, structural proofs, deterministic public naming, overrides, definition validation, IR/lowering and emitted facade. Its `src/` must not import the adapter or the raw backend.

OpenAPI and canonical Bindings are authoritative for wire and structural behavior. `PublicSdkSurface` supplies public-path naming evidence, not a substitute for transport metadata; `SdkOverrides` supplies bounded explicit decisions and exclusions. An unproven operation is reported as rejected instead of being fabricated or made valid by weakening structural checks. A complete explicit definition can also be passed to `generate()`, subject to the same validation.

The consumer owns its chosen OpenAPI revision, public naming evidence, approved exclusions, minimal or production runtime, packaging and release gates. The generated facade is not by itself a complete HTTP stack or published crate. The [independent SDK example](../examples/independent-sdk/README.md) tests the boundary with a separate fixture, standalone Cargo crate and local mock server; it does not turn the example runtime into a library guarantee.

## Source map

| Area | Responsibility |
| --- | --- |
| `src/openapi.rs`, `src/reconcile.rs`, `src/structural.rs` | OpenAPI indexing, operation identity and structural evidence |
| `src/naming.rs`, `src/derivation.rs`, `src/projection.rs` | Public-path decisions, exhaustive outcomes and definition construction |
| `src/contracts.rs`, `src/validation.rs` | Typed contracts and fail-closed validation |
| `src/lower.rs`, `src/ir.rs`, `src/emit.rs`, `src/compiler.rs` | Closed lowering, deterministic files and inventory |
| `src/main.rs`, `src/output.rs` | CLI and safe publication of generated output |
| `openapi-to-rust-bindings/src/` | Manifest-free Rust/effective-OpenAPI adapter and Bindings validation |
| `tests/`, `examples/independent-sdk/` | Generic fixtures, unit/integration contracts and standalone SDK proof |

Keep raw-generator schema changes in the adapter, generic SDK behavior in the root generator, and application-specific policies in the consumer. A missing backend capability is not evidence that an OpenAPI operation may be silently discarded.
