# TCP JSONL Stream Frames

Leash runtime streams default to in-process `local-pubsub` or deterministic
`memory` backends. External module processes can use the smaller TCP JSONL
boundary without changing those defaults.

Each line is one `NetworkStreamFrame`:

```json
{"schema_version":"leash-stream-jsonl-v1","stream":"telemetry","payload":{"seq":1}}
```

The public helpers are:

- `NetworkStreamFrame`
- `write_network_stream_frame`
- `read_network_stream_frame`
- `send_tcp_jsonl_stream_message`
- `accept_tcp_jsonl_stream_message`

The loopback test in `src/transport.rs` is the executable example: it binds a
localhost `TcpListener`, sends one `StreamMessage`, and verifies the received
message matches exactly. This is a framing contract for cross-process modules,
not a long-lived distributed runtime supervisor.
