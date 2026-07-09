# Message Schemas

Leash publishes JSON Schema for the wire messages external tools consume:

- HTTP health, capabilities, telemetry, stream frames, and stop responses
- MCP HTTP status, tool list, module map, and call responses
- capability descriptors, safety classes, module graph messages, and adapter messages
- perception, visualization, planner, patrol, spatial-memory, drone, and manipulator payloads
- network stream frames for TCP JSONL cross-process module boundaries
- versioned worker input/output frames and sanitized worker health status

The canonical artifact is [schemas/leash-messages.schema.json](../schemas/leash-messages.schema.json).
It is generated from Rust `serde` + `schemars` types:

```bash
cargo run --features mcp --bin leash-schema -- --output schemas/leash-messages.schema.json
```

CI runs the generator in `--check` mode. If a Rust wire type changes without
updating the checked-in schema, CI fails.

## Compatibility Rules

`schema_version` is the external message contract version. Change it when a
consumer must update code to keep parsing messages safely, including field
removal, field rename, enum value removal or rename, or a required-field change.

Backward-compatible changes keep the same `schema_version`: adding optional
fields, adding fields with serde defaults, adding new schemas, widening numeric
ranges, or adding enum values that clients can safely ignore. Consumers should
ignore unknown object fields and switch on known enum values with an explicit
fallback path.

Versioned payloads such as `visualization.version` and manipulator `version`
stay scoped to that nested payload. A nested payload version bump does not
require a top-level `schema_version` bump unless the cross-message contract also
breaks.

`NetworkStreamFrame.schema_version` is scoped to the TCP JSONL stream frame. A
network frame version bump does not require changing the top-level schema
artifact version unless the broader message bundle becomes incompatible.
