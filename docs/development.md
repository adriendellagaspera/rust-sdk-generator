# Development and quality gates

The public independent SDK quickstart needs Rust 1.88+ with Cargo and Git;
Python 3.11+ is used only for internal policy, lint and adapter integration
checks. The pinned backend and consumer build also require first-run network
access for source and Cargo dependencies.

```sh
python3 scripts/check_agent_contract.py
cargo fmt --all -- --check
cargo clippy -p rust-sdk-generator --all-targets --all-features -- -D warnings
cargo test -p rust-sdk-generator --all-targets --all-features
cargo clippy -p openapi-to-rust-bindings --all-targets --all-features -- -D warnings
cargo test -p openapi-to-rust-bindings --all-targets --all-features
uvx --from ruff==0.16.8 ruff check --select E4,E7,E9,F63,F7,F82 tests/test_bindings_integration.py scripts openapi-to-rust-bindings/scripts examples/independent-sdk/prove.py
```

The Python adapter-to-CLI integration also needs both executable artifacts built; see `.github/workflows/ci.yml`. The [standalone SDK example](../examples/independent-sdk/README.md) gives reproduction commands for the pinned unmodified upstream backend, manifest-free Bindings v3, exhaustive derivation report, deterministic emitted files, standalone compilation and mock HTTP behavior tests. Run the full CI before merge: the native quickstart proves onboarding, while the required `production-compatibility` job also checks the versioned support envelope and fail-closed drift. `.github/workflows/openapi-to-rust-compat.yml` resolves upstream baseline/candidate revisions immutably on pull requests, nightly schedule and manual dispatch; the manifest-era fork oracle is manual and optional.

When reviewing a change, establish the owning boundary first. For behavioral changes, add a minimal generic fixture or a focused regression test, preserve explicit rejection of unproven operations, and check the generated source and public inventory for intended changes. For backend layout changes, update the manifest-free adapter evidence and production compatibility fixtures, not the root generator; legacy manifest fixtures belong only to the explicit historical oracle. For runtime, source tracking or distribution decisions, update the consumer repository instead.

A green lint/test suite is necessary but does not prove every OpenAPI feature or a production-ready runtime. Generated SDK HTTP integration is demonstrated only for operations exercised by the independent example; unsupported structures remain explicit in derivation reports. Avoid broad clippy suppressions and long-lived stale migration comments when changing the owning module.
