# External Client Examples

These examples use only language standard libraries. Start sim HTTP first:

```bash
cargo run -- serve http --profile sim --listen 127.0.0.1:8000
```

Then run either client:

```bash
LEASH_URL=http://127.0.0.1:8000 python3 examples/clients/python/http_client.py
LEASH_URL=http://127.0.0.1:8000 node examples/clients/node/http-client.mjs
```

Each client reads `/health`, consumes `/telemetry`, and invokes `POST /stop`.
