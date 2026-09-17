# Agent contract

This repository contains a backend-neutral Rust SDK generator and the concrete
`openapi-to-rust-bindings` adapter. Keep this file short: it is an index of
boundaries and executable contracts, not a second implementation spec.

## Repository map

- `src/rust_sdk_generator/`: backend-neutral SDK derivation and Rust emission.
- `tests/`: generator fixtures and tests that do not require a concrete backend.
- `openapi-to-rust-bindings/`: conversion from `openapi-to-rust` output to normalized `Bindings`.
- `.github/workflows/`: required automation and quality gates.
- `scripts/`: small repository-policy checks used locally and in CI.

## Canonical local checks

Run the same focused contracts CI runs rather than inventing ad-hoc validation:

```sh
python3 scripts/check_agent_contract.py
uvx --from ruff==0.16.8 ruff check --select E4,E7,E9,F63,F7,F82 src tests scripts openapi-to-rust-bindings/src openapi-to-rust-bindings/tests
uv build --wheel
python -m unittest discover -s tests -p 'test_*.py'
(cd openapi-to-rust-bindings && uv build --wheel)
```

Use the full GitHub CI before merge for the isolated wheel-install and generic
integration checks.

## Gated invariants

- [policy] `CLAUDE.md` MUST contain only `@AGENTS.md`, and root agent instruction files MUST stay within the combined 150-line budget.
- [policy] Nested `AGENTS.md` or `CLAUDE.md` files MUST NOT be added; repository-specific detail belongs in code, tests, README material, or an executable gate.
- [policy] Third-party GitHub Actions MUST use immutable full commit SHAs.
- [policy] Workflows MUST NOT use `pull_request_target`.
- [policy] Pull-request titles MUST follow the repository's conventional title grammar.
- [generator] The root generator MUST remain independent of `openapi-to-rust` and `openapi-to-rust-bindings` implementation details.
- [generator] Generic generator behavior MUST be demonstrated with non-Mistral fixtures.
- [bindings] `openapi-to-rust-bindings` MUST only translate backend output/metadata into normalized `Bindings`; public SDK policy belongs in the root generator.
- [integration] The generic adapter-to-generator boundary MUST remain covered end to end without a `mistralai-rs` checkout.
- [gate] Required CI jobs MUST converge on the single `gate` conclusion job before merge.

## Working guidance

Prefer the smallest change at the owning boundary. Preserve deterministic output
unless a change explicitly owns an output migration. Add a regression fixture for
behavioral bugs. Keep comments for non-obvious constraints, provenance, invariants,
or rationale; do not narrate straightforward code.

Do not copy Mistral-specific orchestration, taxonomy, endpoint names, or source
update policy into this repository. Consumer policy remains in consumer repositories.
