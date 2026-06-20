# Leash Schemas

`leash-messages.schema.json` is generated from Rust wire types with:

```bash
cargo run --features mcp --bin leash-schema -- --output schemas/leash-messages.schema.json
```

CI checks the file with:

```bash
cargo run --features mcp --bin leash-schema -- --check
```

The top-level `schema_version` changes only when the external message contract
has a breaking change. Additive optional fields and fields with serde defaults
are backward compatible under the same version.
