# Public façade closure

The public SDK is closed when every type reachable from a public model, request,
resource method, response, or stream has a stable public representation. Backend
symbols listed in `Bindings.symbol_paths` must not occur in those signatures, even
inside `Option`, `Vec`, maps, stream items, enum payloads, trait bounds, tuples, or
arrays. A qualified backend path is still a backend symbol.

`inspect_public_facade(&GenerateInput)` lowers the normal generic definition and
returns a deterministic `FacadeReport`. Each leak names its public member, its
signature component, the backend symbol, and its qualified path. The
`consumer_leaks()` iterator isolates unprojected consumer parameters and results;
`TransportInterop` records the currently public raw conversion methods and trait
implementations. `validate_public_facade` (or `report.require_closed()`) is the
strict gate, returning `facade.raw_symbol_exposed` with the first member's path.
The CLI provides `rust-sdk-generator audit --openapi FILE --bindings FILE
--definition FILE` for the full JSON report; `--strict` also exits with status 1
when leaks remain.

The audit runs on the compiler IR before emission. It does not inspect rendered
Rust or infer wire compatibility. Generation is intentionally permissive until
request/response projection can close existing SDKs. At that point, invoke the
strict gate in `compiler::compile` and change transport helpers (`raw`, `as_raw`,
`into_raw`, `from_raw`, and backend `From` implementations) to `pub(crate)` or
private helpers. Trait implementations exposing backend types cannot be made
`pub(crate)` in Rust: replace them with internal conversion functions where they
are needed. Do this together with projection so internal calls keep compiling.

Current consumer work still includes raw request parameters and generated
parameter request fields, unprojected response and stream payloads, public enum
payloads and model constructor/setter/accessor types, and raw client/model escape
hatches. The report distinguishes the precise remaining members for a particular
SDK input; this generator feature does not add consumer-specific policies.
