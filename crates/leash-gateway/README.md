# leash-gateway

This crate owns wire DTOs and conversion into the stable Leash domain API. It
does not own policy, hardware, an async runtime, HTTP, MCP, CLI parsing, ROS, or
CUDA. Those surfaces can all call `TypedCommandService` and serialize the same
`CommandResponse` contract. Its associated-type `CommandService` and
`QueryService` contracts also let frozen compatibility surfaces share a typed
facade without importing their application types into this dependency leaf.

Stop and e-stop bypass the normal proposal lane. They return as soon as the
non-dropping safety mailbox accepts the request. Other commands wait for the
deterministic transition ticket with a caller-configured timeout.

The legacy application exposes that compatibility facade as
`leash_harness::TransportGateway`. HTTP handlers and the MCP dispatcher use it
directly; the local CLI MCP command uses the same dispatcher. HTTP and MCP own
their request decoding and schema rendering, while the service owns policy-safe
execution and concrete response types.
