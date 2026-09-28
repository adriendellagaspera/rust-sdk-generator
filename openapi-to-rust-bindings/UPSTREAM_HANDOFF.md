# Upstream handoff: generic producer improvements

This file contains narrowly scoped handoffs for `gpu-cli/openapi-to-rust`. They concern ordinary generated Rust or CLI behavior, not a consumer-specific binding manifest. Existing upstream discussions should be reused rather than duplicated: [#77](https://github.com/gpu-cli/openapi-to-rust/issues/77) (request-local multipart filenames), [#78](https://github.com/gpu-cli/openapi-to-rust/issues/78) (response representations and live binary streams), and [#79](https://github.com/gpu-cli/openapi-to-rust/issues/79) (Overlay 1.1). The production metadata baseline is `gpu-cli/openapi-to-rust` v0.19.0 (`e5fc983c02dae5a09bcb08ebdf5669f6ae1f624f`). Structured bindings metadata was introduced by upstream issue #80; the follow-ups below are deliberately narrow additions to that contract.

## Issue draft: repeated scalar and nullable binary multipart fields

**Title:** `[bug]: Handle repeated scalar and nullable binary multipart fields`

**Actual:** For `multipart/form-data`, an object field whose schema is `type: array` of text or string-enum values does not generate repeated `Form::text` calls. A binary property wrapped in `anyOf: [$ref(binary), {type: null}]` does not reliably retain the binary multipart `Part` path.

**Expected:** Encode each scalar array element as a separate form field with the same original wire name; unwrap the unambiguous non-null schema branch for a nullable binary field and emit a binary part. Preserve the historical behavior of unrelated fields and absent optional fields.

**Minimal fixture:** [`fork-producer-delta/openapi.json`](tests/producer-delta/openapi.json), operations `createUpload` and schemas `UploadRequest`/`File`/`Mode`. The fixture also includes independent fields for other checks; the multipart subset alone is sufficient for this issue.

**Candidate implementation/test:** [fork commit `216ab55`](https://github.com/adriendellagaspera/openapi-to-rust/commit/216ab550b6e84db3601386acf123c246896eab50), especially `tests/client_multipart_capabilities_test.rs`. The pinned upstream and fork outputs are directly compared in `fork-manifest-removal-evidence.yml`; the capability requires examining the *generated form-building code*, not just the OpenAPI declaration.

**Reproduction:**

```sh
cargo run --locked --bin openapi-to-rust -- generate path/to/openapi.json \
  --output-dir /tmp/multipart-output --module-name multipart --quiet
rg 'form = form.text\("labels", item.to_string\(\)\)' /tmp/multipart-output/client.rs
```

The generator should emit the repeated-field loop for the `labels` array, and must retain `reqwest::multipart::Part::bytes` for the nullable binary `file` field.

## Small upstream PR candidate: collision-stable inline parameter enum names

Use [fork commit `d19e5a4`](https://github.com/adriendellagaspera/openapi-to-rust/commit/d19e5a4cba2589475dc157e50fe624576f129368) as a narrowly scoped patch, with `tests/client_param_enum_collision_test.rs`. The source `enum: [created, -created]` normalizes both variants to `Created`; generated names should remain collision-safe and conventionally PascalCase (`Created`, `Created2`) with distinct `serde(rename)` values. Do not include manifest-related work or change consumer naming policy. Upstream [PR #19](https://github.com/gpu-cli/openapi-to-rust/pull/19) addressed broader enum collision handling; this candidate concerns only the concrete suffix spelling, and should be rebased after reviewing the current renderer.

## Small upstream PR candidate: multiline property rustdoc

Use [fork commit `f7060dc`](https://github.com/adriendellagaspera/openapi-to-rust/commit/f7060dc7fef6c2dfd73b65713e8fa2d9a1c4a0eb) as a one-renderer-hunk patch with `tests/property_multiline_doc_test.rs`. A property with a description containing newlines should generate one `#[doc = ...]` attribute per logical line so rustdoc keeps readable paragraph boundaries. Include only renderer change and narrowly scoped generated-Rust snapshot(s), not fork corpus-hash updates or metadata work.


## Bindings metadata follow-ups required to delete the compatibility shim

These are follow-ups to upstream #80. They describe producer-owned facts already known while rendering the HTTP client. None should reference `rust-sdk-generator` or require a consumer-specific schema.

### Issue draft: exact request parameter wire mappings

**Title:** `[feature]: Expose exact request parameter wire mappings in bindings metadata`

Bindings metadata v1 exposes exact method signatures and source-operation identities, but not the mapping from allocated Rust arguments to query/header wire names. That mapping becomes ambiguous after identifier normalization or collision suffixing, for example `sort.direction` and `sort_direction`.

Expose the actual mapping from the client emission plan, e.g. `{ rust_name, location, wire_name }` per request parameter. This lets metadata consumers use the emitted API without parsing request-building Rust or reimplementing name allocation.

### Issue draft: fixed request effects for generated method variants

**Title:** `[feature]: Expose fixed request effects for generated method variants in bindings metadata`

One source operation can produce multiple methods whose call shapes differ because the generator injects request values, for example a streaming variant setting a request field to `true` before serialization.

Expose these fixed effects from the method plan in a generic representation such as `fixed_field { rust_access_path, wire_name, rust_type, value }`. The contract should describe generator behavior generically rather than infer semantics from method names.

### Issue draft: HTTP client construction roles

**Title:** `[feature]: Expose HTTP client construction roles in bindings metadata`

Metadata v1 records public methods and signatures, but consumers still need backend-specific knowledge to identify the HTTP client type and the methods that construct/configure it.

Expose explicit semantic roles for the emitted client type, constructor, base-URL configuration and authentication configuration. Exact names may remain values of those roles; consumers should not have to identify them by convention or inspect method bodies.

### Issue draft: structured ownership and transport metadata for live response streams

**Title:** `[feature]: Expose structured ownership and transport metadata for live response streams`

Metadata v1 exposes exact return signatures and response consumption, but consumers still have to parse Rust type syntax to establish stream item/error types, ownership/lifetime and whether the transport is a named alias or anonymous `impl Trait`.

For live response methods, expose those properties structurally from the same response/method plan used for emission. Keep the exact Rust signature as well. This complements the raw-stream lifetime fix: metadata should report the ownership/transport actually emitted.

Once these four gaps are represented upstream, `openapi-to-rust-bindings` should be deleted rather than extended.
