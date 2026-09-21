# Upstream handoff: generic producer improvements

This file contains narrowly scoped handoffs for `gpu-cli/openapi-to-rust`. They concern ordinary generated Rust or CLI behavior, not a consumer-specific binding manifest. Existing upstream discussions should be reused rather than duplicated: [#77](https://github.com/gpu-cli/openapi-to-rust/issues/77) (request-local multipart filenames), [#78](https://github.com/gpu-cli/openapi-to-rust/issues/78) (response representations and live binary streams), and [#79](https://github.com/gpu-cli/openapi-to-rust/issues/79) (Overlay 1.1). The pinned upstream baseline is `5a3487edbe27cfd4efb32dda893774e23d7fa195`.

## Issue draft: repeated scalar and nullable binary multipart fields

**Title:** `[bug]: Handle repeated scalar and nullable binary multipart fields`

**Actual:** For `multipart/form-data`, an object field whose schema is `type: array` of text or string-enum values does not generate repeated `Form::text` calls. A binary property wrapped in `anyOf: [$ref(binary), {type: null}]` does not reliably retain the binary multipart `Part` path.

**Expected:** Encode each scalar array element as a separate form field with the same original wire name; unwrap the unambiguous non-null schema branch for a nullable binary field and emit a binary part. Preserve the historical behavior of unrelated fields and absent optional fields.

**Minimal fixture:** [`fork-producer-delta/openapi.json`](tests/fixtures/fork-producer-delta/openapi.json), operations `createUpload` and schemas `UploadRequest`/`File`/`Mode`. The fixture also includes independent fields for other checks; the multipart subset alone is sufficient for this issue.

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
