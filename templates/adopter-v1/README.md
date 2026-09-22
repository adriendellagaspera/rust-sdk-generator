# Reviewed standalone SDK starter, version 1

Only `src/lib.rs` and `src/sdk/error.rs` are source templates. Both become
consumer-owned on init and are never overwritten on sync. The upstream-specific
`ApiOpError` mapping is localized in `error.rs`; it is a minimal compile-time
bridge and not a production error taxonomy or universal backend abstraction.
The root generator's current Runtime default selects the exported error aliases.

The adopter creates consumer-owned `Cargo.toml` from the producer's exact
`REQUIRED_DEPS.toml` plus documented facade/test dependencies. It leaves
`publish = false`, no license, and no production authentication or publishing
policy. Review/replace the runtime, public crate identity, dependencies, network
behavior, license, and errors before shipping. Changing this template requires
an explicit versioned recipe migration; it must never silently rewrite code
previously owned by a consumer.
