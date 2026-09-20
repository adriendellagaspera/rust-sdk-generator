# #148 — Manifest-free upstream Bindings: evidence and feasibility

Status: **audit / bounded prototype**, not a production adapter. Parent: #147; implementation: #149, #150; cutover: #151.

## Immutable comparison and reproducibility

- Unmodified upstream: [`gpu-cli/openapi-to-rust@5a3487edbe27cfd4efb32dda893774e23d7fa195`](https://github.com/gpu-cli/openapi-to-rust/tree/5a3487edbe27cfd4efb32dda893774e23d7fa195), v0.17.0-era default branch.
- Fork: [`adriendellagaspera/openapi-to-rust@9fee7af67c05d896da86e41d326b067312120c53`](https://github.com/adriendellagaspera/openapi-to-rust/tree/9fee7af67c05d896da86e41d326b067312120c53).
- [Exact upstream → fork comparison](https://github.com/adriendellagaspera/openapi-to-rust/compare/5a3487edbe27cfd4efb32dda893774e23d7fa195...9fee7af67c05d896da86e41d326b067312120c53): 13 commits on the fork, no divergence from this upstream baseline. This is a **dated snapshot**, not a claim about newer upstream revisions.
- Existing adapter pins fork `d19e5a4cba2589475dc157e50fe624576f129368`, **not** the fork head above: [adapter pin](../../openapi-to-rust-bindings/COMPATIBILITY.json). Pin changes and parity comparisons must be explicit.
- Inputs to the prototype are **the original effective OpenAPI JSON** (overlays already applied), the raw backend's ordinary `client.rs`/`types.rs` outputs, and the immutable backend revision/configuration. Do not silently reconstruct overlays or assume an emitted method covers every declared response variant.
- Run [the bounded upstream probe](../../openapi-to-rust-bindings/scripts/probe_upstream_evidence.py) with the commands below. It intentionally does **not** produce Bindings or parse all Rust; #149 replaces it with a Rust AST extractor.

## Fork delta classification

| Fork commit | Classification | Migration consequence |
| --- | --- | --- |
| [216ab55](https://github.com/adriendellagaspera/openapi-to-rust/commit/216ab550b6e84db3601386acf123c246896eab50) repeated/nullable multipart | **B — raw behavior** | Upstream may lack this transport capability. Adapter cannot synthesize it. |
| [37cc2f3](https://github.com/adriendellagaspera/openapi-to-rust/commit/37cc2f3580038b0c45b94fc69fa6528b68259699) per-call multipart filenames | **B — raw behavior** | Additional real method/parameter; preserve separately if needed. |
| [2ff70d1](https://github.com/adriendellagaspera/openapi-to-rust/commit/2ff70d10aa25d51e6b9de44aeb118729e8a052c1) owned SSE | **B — raw behavior** | The old upstream return/lifetime may not satisfy the facade's ABI. |
| [041cc44](https://github.com/adriendellagaspera/openapi-to-rust/commit/041cc445775d34ebe7d9078c88d876fa8379b637) binary streaming | **B — raw behavior** | Additional method/stream type, unavailable until backend emits it. |
| [2e083e5](https://github.com/adriendellagaspera/openapi-to-rust/commit/2e083e5e0c391cd5fd205e119b5aa29fdf5b639) retain response representations | **B — raw planning** | Enables observing/generating alternate transports rather than guessing from OpenAPI. |
| [7aa4411](https://github.com/adriendellagaspera/openapi-to-rust/commit/7aa4411b7101f34a05380ffc8caf2ee31b988d23) call-shape planning | **B — raw behavior/planning** | Shared generation planning matters; do not copy backend decision algorithms into the adapter. |
| [e13969c](https://github.com/adriendellagaspera/openapi-to-rust/commit/e13969c7300f89e2d6ce7f591295db1dd1bb0748) overlays | **C — upstream input/preprocessing feature** | Use a supplied, explicitly materialized effective OpenAPI for the adapter; overlay application is not Bindings extraction. |
| [25831bd](https://github.com/adriendellagaspera/openapi-to-rust/commit/25831bd6f96d48be8d8613fde3e92bc3323c3aa6) request discriminators | **B — raw request behavior** | Only mark discriminator metadata if emitted assignments are proven. |
| [addb1a6](https://github.com/adriendellagaspera/openapi-to-rust/commit/addb1a6c69d63cc83178dca1c9f749b03711bc54) manifest v1 | **A — producer instrumentation, plus shared planning refactor** | Remove dedicated manifest emission/serialization dependency, **not** generic code-path corrections or shared call-shape planning. Review the mixed commit at hunk level. |
| [d19e5a4](https://github.com/adriendellagaspera/openapi-to-rust/commit/d19e5a4cba2589475dc157e50fe624576f129368) inline enum name collision | **B — raw emitted name fix** | Affects actual type names referenced by generated code. |
| [f7060dc](https://github.com/adriendellagaspera/openapi-to-rust/commit/f7060dc7fef6c2dfd73b65713e8fa2d9a1c4a0eb) multiline doc comments | **C — raw formatting fix** | Parser must not depend on incidental doc formatting. |
| [60441a8](https://github.com/adriendellagaspera/openapi-to-rust/commit/60441a884bc431b53da9a2d916e72a8766d1a9cb) client parameter enums in manifest | **A — manifest completeness**, with shared helper extraction | Ordinary `client.rs` carries the enum declarations; preserve any name-allocation correction independently. |
| [9fee7af](https://github.com/adriendellagaspera/openapi-to-rust/commit/9fee7af67c05d896da86e41d326b067312120c53) exact source paths | **A — provenance correction for manifest**, plus analysis bookkeeping | The emitted wire route may differ from the original OpenAPI path key: recover source identity from the **effective OpenAPI**, never substitute the generated wire route. |

**A** = removal candidate only for metadata-specific code; **B** = genuine generated-Rust capability/behavior, not removable by adapter migration; **C** = unrelated producer concerns. A commit can mix categories. Reverting whole commits is **not** an acceptable removal strategy.

## Bindings v3 evidence matrix

Notation: **D** = declaration/AST evidence available from ordinary raw output; **B** = requires inspection of generated method behavior; **I** = requires correlation with the exact effective OpenAPI; **U** = unavailable or ambiguous in a stated case. “Potential” means a design hypothesis for #149/#150, **not an implemented extractor or proven parity**. Source paths refer to the selected output configuration, not hard-coded backend locations.

| Bindings field / capability | Producer-independent evidence and proposed extractor | Baseline confidence / hard case | Fail-closed outcome if unproven |
| --- | --- | --- | --- |
| `structs`, Rust fields and types | D: `syn` public item/field AST from generated module tree; use emitted names/types | High for ordinary declared fields; extra serde flatten/visibility require explicit support | `bindings.extract.unhandled_model_shape` |
| Field `wire_name` | D: inspect actual `#[serde(rename = ...)]` and rename rules; otherwise declared field spelling only when serde's effective name is provable | Medium; custom `serialize_with`/flatten cannot be inferred from source schema alone | `bindings.extract.wire_name_unproven` |
| `enums`, variants, payloads, wire names | D: enum AST, `serde` attributes, effective rename rules | Medium; unit/untagged/discriminated/external tag vary | `bindings.extract.enum_encoding_unproven` |
| `aliases`, `symbol_paths`, client-parameter enums | D: module traversal, type aliases, enums in `client.rs` as well as `types.rs` | High for public declarations, including renamed collision allocation | `bindings.extract.symbol_unresolved` |
| Raw client type, constructor, auth/base-url builders, preludes | D: inspect client impl methods and module paths; do **not** hard-code `HttpClient` as general interface | Medium, backend/config/version-specific | `bindings.extract.client_layout_unproven` |
| Method name, parameter **order/types**, Rust return/success type | D: `syn::ImplItemFn` signatures and resolved generic/type aliases | High for signature tokens; success type may depend on return wrappers and aliases | `bindings.extract.signature_unsupported` |
| Source `operationId`, HTTP verb, **original OpenAPI path** | B + I: match emitted HTTP verb and effective wire URL to exactly one path+verb; take original ID/path **from OpenAPI**, never from Rust name. Compare actual method body, not only docs. | Medium for unique literal routes; **U** for equivalent normalized routes, aliases, custom URL construction, ambiguous selectors/collisions | `bindings.extract.source_identity_ambiguous` |
| Emitted operation ID after allocator renames | I + corroborated producer evidence if available; Rust method name does **not** prove intermediate emitted ID | **U** when ID is not preserved in ordinary output/accessible supported interface | `bindings.extract.emitted_id_unproven` |
| Multiple call shapes per source operation | D + B + I: enumerate real public methods; correlate each method's transport separately | Upstream v0.17.0 has no fork's alternate call-shape planning; OpenAPI alternatives alone do not prove emitted variants | `bindings.extract.call_shape_unproven` |
| JSON/text/empty/buffered binary/event/binary stream representation and media type | D + B + I: success type, actual body decoding/streaming, request `Accept`, relevant OpenAPI media description | Medium for simple unambiguous methods; **U** for mixed media without observable selection or custom code | `bindings.extract.representation_unproven` |
| Exact accepted success statuses | B: inspect actual status guards/branches, distinguish `status.is_success()` from a set of declared 2xx statuses | Medium for supported generated patterns; OpenAPI alone is insufficient | `bindings.extract.success_statuses_unproven` |
| Stream ABI (alias, item/error/lifetime, native/WASM) | D + B: declared aliases/return types and cfg-specific implementations; compile both targets where required | **U** for upstream `impl Stream` if the complete owned ABI/lifetime cannot be proven | `bindings.extract.stream_abi_unproven` |
| Multipart filename helper + extra args | D + B: require emitted public helper and its actual request-body path | **Not generated by baseline upstream** in this form; cannot be manufactured | `bindings.extract.helper_not_emitted` |
| Discriminator wire key/path/type/value and nullability/tri-state | B + D + I: inspect actual request assignments, resolve field accesses and serde wire mapping; match selected transport | **Not generated by baseline upstream** via fork's config feature; do not infer from an OpenAPI `stream` property | `bindings.extract.discriminator_unproven` |
| Missing operations / closed-world report | I: enumerate all effective OpenAPI operations, compare to proven method identities and report rejected/unemitted separately | A valid Bindings object alone does **not** certify full source coverage | `bindings.extract.source_operation_unemitted` |

These are proposed **stage-specific diagnostics** for #149/#150, not error codes emitted by today's adapter. Bindings v3 still requires the validated canonical identity/transport metadata for supported operations: do not silently downgrade to v2 to disguise missing evidence. See [typed input](../../src/contracts.rs), [validation](../../src/validation.rs) and [existing manifest normalizer](../../openapi-to-rust-bindings/src/manifest.rs).

## Bounded upstream-only spike

The [research-only probe](../../openapi-to-rust-bindings/scripts/probe_upstream_evidence.py) consumes ordinary upstream output and one [minimal fixture](../../openapi-to-rust-bindings/tests/fixtures/upstream-evidence/openapi.json). It tests what a literal-path/HTTP-verb correlation can and **cannot** establish. It deliberately does not generate Bindings or claim to be a Rust AST parser.

```sh
git clone https://github.com/gpu-cli/openapi-to-rust.git upstream
git -C upstream checkout --detach 5a3487edbe27cfd4efb32dda893774e23d7fa195
cargo build --locked --release --manifest-path upstream/Cargo.toml --bin openapi-to-rust
SPEC="$PWD/openapi-to-rust-bindings/tests/fixtures/upstream-evidence/openapi.json"
OUT="$(mktemp -d)"
upstream/target/release/openapi-to-rust generate "$SPEC" --output-dir "$OUT" --module-name upstream_probe --quiet
test ! -e "$OUT/binding-manifest.json"
python3 openapi-to-rust-bindings/scripts/probe_upstream_evidence.py --openapi "$SPEC" --generated "$OUT"
```

The [dedicated PR workflow](../../.github/workflows/upstream-evidence-spike.yml) automates this proof with a detached pinned upstream checkout. [The first upstream-only execution passed](https://github.com/adriendellagaspera/rust-sdk-generator/actions/runs/35543645933): the SHA check, release build, ordinary Rust generation, absence-of-manifest assertion, probe self-test (renamed ID + four fail-closed negatives) and fixture correlation all succeeded. It does **not** gate ordinary generator CI; the spike is a bounded research gate for #148.

**Observed output of that pinned upstream run:** exactly three public operation methods. `GET /inventory` was correlated with the source `fetchInventoryWithoutNamingShortcut` and Rust `fetch_inventory_without_naming_shortcut`; its return was `Result<Inventory, ApiOpError<serde_json::Value>>`. `POST /render` declared both `application/json` and `text/event-stream` but emitted **one** method returning `Inventory`, with no observed `bytes_stream()` call or SSE `Accept` header. `GET /events` emitted a method returning `impl futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>>`, with an observed `bytes_stream()` call and SSE `Accept` header. These are **bounded source observations**, not proof of complete body semantics, accepted status lists, canonical emitted IDs, owned/static stream ABI or Bindings v3 parity. See the run's `Generate ordinary raw outputs and inspect source evidence` log for the full machine-readable report.

### Existing source-level findings and decisions

1. Upstream's [single-method generator](https://github.com/gpu-cli/openapi-to-rust/blob/5a3487edbe27cfd4efb32dda893774e23d7fa195/src/client_generator.rs#L1417-L1470) emits one method per operation, with an HTTP verb and URL in its body. Its [response selector](https://github.com/gpu-cli/openapi-to-rust/blob/5a3487edbe27cfd4efb32dda893774e23d7fa195/src/client_generator.rs#L2912-L2985) chooses a preferred response; it does not emit the fork's independent JSON/SSE or buffered/stream call shapes. **Missing alternative methods = missing producer capability, not a parser defect.**
2. The [upstream method naming routine](https://github.com/gpu-cli/openapi-to-rust/blob/5a3487edbe27cfd4efb32dda893774e23d7fa195/src/client_generator.rs#L2286-L2303) transforms IDs. Rust method names therefore cannot be used as canonical source identity; only a **uniquely proven** verb+wire-path match to effective OpenAPI is a candidate. [Fork source-path correction](https://github.com/adriendellagaspera/openapi-to-rust/commit/9fee7af67c05d896da86e41d326b067312120c53) demonstrates why the wire route may not equal the original key.
3. The [upstream success selection](https://github.com/gpu-cli/openapi-to-rust/blob/5a3487edbe27cfd4efb32dda893774e23d7fa195/src/client_generator.rs#L2912-L3007) can select JSON over SSE when both are declared; do not manufacture SSE Bindings from the spec. The generated method's `Accept`, decoder, stream path and status predicate must jointly support any claimed representation.
4. Upstream may return an [`impl Stream` signature](https://github.com/gpu-cli/openapi-to-rust/blob/5a3487edbe27cfd4efb32dda893774e23d7fa195/src/client_generator.rs#L3030-L3053); the fork separately added [owned static SSE ABI](https://github.com/adriendellagaspera/openapi-to-rust/commit/2ff70d10aa25d51e6b9de44aeb118729e8a052c1). **An adapter cannot confer a lifetime or expose a second stream method that upstream did not emit.**
5. [Raw fork discriminators](https://github.com/adriendellagaspera/openapi-to-rust/commit/25831bd6f96d48be8d8613fde3e92bc3323c3aa6) and [multipart helpers](https://github.com/adriendellagaspera/openapi-to-rust/commit/37cc2f3580038b0c45b94fc69fa6528b68259699) are real generation features: their absence in upstream is a **backend capability gap**, not missing evidence.

### Design decision for #149 / #150

Introduce a Rust-native `OpenApiToRustEvidence` (typed, located items + separately proven semantics) and a pure `normalize_to_bindings(evidence)` boundary inside the adapter. Required context: immutable backend revision, effective OpenAPI, output module-root path and generation configuration. Use `syn` AST for declarations/impls and conservative supported-pattern inspection for method bodies; keep source spans/provenance for diagnostics and a strict bijective mapping check. Never load or execute handwritten code in the adapter.

A manifest, when a backend voluntarily provides one, can remain an **optional second evidence source** or test oracle. It must never be the default upstream contract or a requirement for onboarding. **Do not merge a broad parser based solely on this probe:** address every U/medium-confidence row in #149/#150 and verify end-to-end compiled/HTTP parity before cutover in #151.
