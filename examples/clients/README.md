# External Client Examples

These examples use only language standard libraries. Start sim HTTP first:

```bash
cargo run -- serve http --profile sim --listen 127.0.0.1:8000
```

Then run the client:

```bash
LEASH_URL=http://127.0.0.1:8000 node examples/clients/node/http-client.mjs
```

The client reads `/health`, consumes `/telemetry`, and invokes `POST /stop`.
