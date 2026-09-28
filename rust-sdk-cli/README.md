# rust-sdk

Install the OpenAPI-to-Rust SDK generator CLI with `cargo install rust-sdk-cli`.
The installed executable is `rust-sdk`.

Run `rust-sdk init --openapi path/to/api.json --output path/to/sdk --name my-sdk`,
then `rust-sdk sync --crate path/to/sdk --check` after updating its `openapi.json`.

Rust 1.88+, Cargo and Git are required. The first init downloads and compiles the
pinned OpenAPI backend; subsequent runs use the local cache. See the
[repository documentation](https://github.com/adriendellagaspera/rust-sdk-generator/blob/main/docs/own-api-sdk.md)
for the supported source format, review workflow and generated crate ownership.
