# MCP HTTP Compatibility

Leash exposes one standards-compatible MCP Streamable HTTP endpoint and keeps
its original REST-shaped bridge endpoints for existing clients.

## Standard MCP endpoint

Start the localhost server:

```bash
leash serve mcp-http --listen 127.0.0.1:9990
```

`POST /mcp` accepts MCP JSON-RPC 2.0 messages. Leash negotiates the current
`2025-11-25` protocol revision and supports `initialize`, `ping`, `tools/list`,
and `tools/call`. It returns tool failures inside the call result with
`isError: true`; malformed requests and unknown tools return JSON-RPC errors.

Initialize before calling tools:

```bash
curl -sS http://127.0.0.1:9990/mcp \
  -H 'content-type: application/json' \
  -H 'accept: application/json, text/event-stream' \
  --data '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"local-operator","version":"1"}}}'

curl -sS http://127.0.0.1:9990/mcp \
  -H 'content-type: application/json' \
  -H 'accept: application/json, text/event-stream' \
  -H 'mcp-protocol-version: 2025-11-25' \
  --data '{"jsonrpc":"2.0","method":"notifications/initialized"}'
```

List and invoke tools:

```bash
curl -sS http://127.0.0.1:9990/mcp \
  -H 'content-type: application/json' \
  -H 'accept: application/json, text/event-stream' \
  -H 'mcp-protocol-version: 2025-11-25' \
  --data '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'

curl -sS http://127.0.0.1:9990/mcp \
  -H 'content-type: application/json' \
  -H 'accept: application/json, text/event-stream' \
  -H 'mcp-protocol-version: 2025-11-25' \
  --data '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"health","arguments":{}}}'
```

The endpoint is stateless and returns JSON rather than opening an SSE response.
`GET /mcp` returns `405 Method Not Allowed`, because Leash does not initiate
server-to-client messages. Browser requests with an `Origin` header are accepted
only from loopback origins to protect the local operator from DNS rebinding.

The MCP transport does not bypass Leash policy. Physical motion still requires
the compiled hardware feature, the runtime physical-actuation gate, and the
configured token or approval policy.

## Existing REST compatibility

These stable JSON endpoints remain available:

- `GET /mcp/status`
- `GET /mcp/tools` and `GET /mcp/list-tools`
- `GET /mcp/modules`
- `POST /mcp/call` with `{"tool":"health","args":{}}`

The unprefixed `/status`, `/tools`, `/list-tools`, `/modules`, and `/call` aliases
also remain available on the dedicated MCP HTTP server. `leash mcp ...` and the
stdio bridge continue to use this compatibility surface.

The module response includes `blueprint` metadata: schema version, profile,
stream transport, hardware requirement, required safety gates, and discoverable
capabilities. No runtime token values or credentials are included.

Protocol behavior follows the official [MCP lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle),
[Streamable HTTP transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports),
and [tools](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)
contracts.
