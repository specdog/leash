# leash-gateway

This crate owns wire DTOs and conversion into the stable Leash domain API. It
does not own policy, hardware, an async runtime, HTTP, MCP, CLI parsing, ROS, or
CUDA. Those surfaces can all call `TypedCommandService` and serialize the same
`CommandResponse` contract.

Stop and e-stop bypass the normal proposal lane. They return as soon as the
non-dropping safety mailbox accepts the request. Other commands wait for the
deterministic transition ticket with a caller-configured timeout.
