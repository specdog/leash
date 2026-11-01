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

## Installed read-only observations

The coordinating agent installed source
`445d56ba8d7009c1c0f04b770590cf0e4b35fc19`, binary SHA-256
`a57a4ae4ecc0eb614d724723c66f0624902916a22bfec2e69ec0a0162a91c027`,
at 21:20:05 EDT. The full eight-feature release cross-build completed in 1m47s;
ELF inspection verified AArch64, GLIBC requirements no newer than 2.34, and
the Cargo fingerprint retained CUDA and the other seven production features.

Three GET observations of the existing host TLS `/belief/status` and
`/belief/sketch` ran from 21:20:47 to 21:20:53 EDT without restarting the host.
All four layers were fresh at the final observation:

| Layer | Initial sequence | Final sequence | Changed activation sketch cells | Changed weight-RMS sketch cells |
| --- | ---: | ---: | ---: | ---: |
| L3 | 404 | 497 | 69 / 256 | 8 / 256 |
| L4 | 109 | 135 | 71 / 256 | 8 / 256 |
| L5 | 27 | 33 | 73 / 256 | 8 / 256 |
| L6 | 6 | 7 | 73 / 256 | 8 / 256 |

The parameter digest changed. The final accepted upstream was canonical v2,
producer epoch `leash:84312:1789262418775496855`, sequence 676, timestamp
1789262453228, expiry 1789262453728. These are actual host state changes;
host capabilities reported `cpu-fallback`, so this is not evidence of host CUDA
learning. Robot cognition separately reported active CUDA without degradation.

Full sanitized observations are at
`C:/Users/ericm/.local/state/qualia/console-runtime/belief-boundary-integration.json`.
The first automatic robot checkpoint, created at Unix ms 1789262478778, still
reported top-down precision 0 and expiry 0. Thus return-channel acceptance was
not established by this observation. The robot ingress credential file is
configured and exists; the host credential file is protected from the observing
user. No credential was printed or copied to an evidence file. The checkpoint
metadata is in `belief-return-checkpoint.json` beside the host observations.
All observations were read-only; no boundary POST, actuator request, or manual
checkpoint was issued.
