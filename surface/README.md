# SDK surface contract

This directory is the Fern-primary replacement path for the responsibilities that remain local after ordinary Rust generation moves to Fern.

\`python3 surface/sdk_surface.py compile\` resolves a small policy keyed by stable OpenAPI source identity and writes deterministic JSON OpenAPI with Fern extensions. \`verify\` compares that resolved surface with Fern IR, checks exact source-operation accounting, method-path collisions, public-method inventory, named-type closure and semantic compatibility. \`digest\` provides a deterministic publishable-tree hash while ignoring \`.fern\` metadata.

The policy intentionally does not describe generated Rust symbols, transport behavior, serialization, multipart/SSE runtime mechanics or backend adapters. Generic Fern defects belong upstream; temporary consumer compatibility rewrites must stay outside this policy and carry an upstream issue/PR plus an explicit removal condition.

Operation identities are \`operationId:<operationId>\` when an operationId exists, otherwise \`http:<METHOD> <path>\`. Type identities are local OpenAPI JSON pointers. Defaults are deliberately limited to deriving resources from the first dotted tag and methods from operationId; explicit overrides remain small and reviewable.

Example:

\`\`\`yaml
schema_version: 1
defaults:
  resource: first_tag_segments
  method: operation_id
operations:
  operationId:chat_completion_v1_chat_completions_post:
    method: complete
    streaming:
      format: sse
      condition: $request.stream
      response_schema: ChatCompletionResponse
      stream_schema: CompletionChunk
types:
  '#/components/schemas/Judge/properties/output':
    name: JudgeOutputConfig
\`\`\`

The compiler emits \`x-fern-sdk-group-name\`, \`x-fern-sdk-method-name\`, \`x-fern-type-name\` and \`x-fern-streaming\`. Excluded operations are removed from the Fern input and remain recorded as reviewed outcomes in the resolution inventory.
