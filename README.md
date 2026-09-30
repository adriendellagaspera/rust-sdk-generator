# rust-sdk-generator

> **Archived project.** This repository is retained for historical reference and is no longer part of the production SDK toolchain.

Fern is now the sole Rust SDK generator. The small surface-policy/compiler verifier that remained useful was moved directly into [adriendellagaspera/mistralai-rs](https://github.com/adriendellagaspera/mistralai-rs) in [mistralai-rs#186](https://github.com/adriendellagaspera/mistralai-rs/pull/186), making that repository self-contained.

Generic Rust generation issues belong upstream in [fern-api/fern](https://github.com/fern-api/fern). No new development is planned here.

The repository history preserves the former OpenAPI-to-Rust generator, Bindings adapters, facade compiler, migration experiments and the minimal Fern surface-contract work that led to the final architecture.
