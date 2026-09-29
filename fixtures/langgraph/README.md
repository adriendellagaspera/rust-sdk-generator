# LangGraph Agent Protocol Fern proof

This fixture is the second independent consumer required by #200/#239.

- Source: `langchain-ai/agent-protocol@fb81f3e27ee507557926ecf923d0f933a1c76d44`
- Input: `openapi.json` (Agent Protocol 0.1.6)
- Expected source operations: 27
- Fern CLI: 5.112.0
- Fern Rust generator: 0.48.0

The fixture contains policy only. CI acquires the pinned OpenAPI source, compiles
that policy to Fern extensions, verifies Fern IR accounting/closure/provenance,
generates twice for deterministic-output proof, and compiles the generated SDK.
