# Installed host cognition boundary compatibility

Read-only diagnosis on 2026-09-12 found host `upstream: null` and all L3-L6
sequences at zero, while robot L0-L2 sequences advanced on CUDA. The configured
Leash URL was already `http://10.0.0.180:8000`; the installed host process had
established connections to that address. The installed release pointer named
Qualia `b2504c8`. Its transcript contained only the startup header.

A direct `/events/cognition` subscription returned HTTP 200 and a fresh frame:
schema `leash.cognition.v1`, source `leash`, destination `planner`, layer 2,
sequence 8169, timestamp 1789261925104, expiry 1789261925604, precision 0.9344344,
1024 latent values, digest
`709bb75a73af103e6e5dbce33d44f2c8bc6eeb218dcf9f9f87f4e4f9e88f7de5`.
The host received it at Unix ms 1789261925119. SSE framing used the expected
`data: ` prefix. The host decoder instead requires a canonical Qualia boundary
schema addressed to `qualia`; both the schema and destination failed validation.

## Narrow wire adaptation

The two implementations have the same L2 latent semantics: 1024 finite f32
values, precision within 0..1, a SHA-256 digest over little-endian f32 bytes,
sequence identity, and a maximum 500 ms boundary lifetime. This is a latent
boundary, not model weights, planner commands, or a camera frame.

`/events/cognition` now emits `qualia.cognition-boundary.v2`, addressed from
`leash` to `qualia`, with a stable per-runtime producer epoch. It preserves the
original latent values, digest, sequence, precision, timestamp, and expiry.
The legacy `/cognition/status` boundary and local checkpoint schemas remain
unchanged; the new epoch field is omitted from legacy JSON.

The authenticated `/cognition/boundary` return path accepts canonical v2 frames
from `qualia` to `leash` at L3. Existing dimension, finite-value, precision,
digest, and freshness checks remain. Canonical frames additionally require a
nonempty bounded producer epoch and nonzero sequence; rollback within an epoch,
conflicting sequence reuse, and epoch changes without newer evidence are refused.
An identical repeated frame is idempotent and does not extend its expiry.
Legacy authenticated `planner` frames retain their existing route.

The adaptation does not grant model output wheel or head authority. No model
weights, motor paths, service configuration, or user root files were changed.
The source was inspected against the installed host decoder without test cycles;
the full production-feature build and live belief integration are separate
deployment evidence.
