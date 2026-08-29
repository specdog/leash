# leash-waveshare

This crate is the single-owner boundary for the Waveshare UGV base controller.
One named thread owns the I/O factory, live duplex stream, reads, writes,
reconnects, and JSONL framing. Callers receive cloneable handles, not serial
objects or mutex guards.

Normal base and gimbal work uses a bounded reject-newest lane. Stop and e-stop
use the non-dropping atomic safety mailbox from `leash-runtime`; the owner
checks it before normal work and flushes queued motion. E-stop latches until the
owner is restarted. The physical reset workflow will later supply a typed reset
permit rather than exposing an unguarded boolean here.

The `ControllerIo` contract requires non-blocking reads or a read timeout no
greater than `OwnerConfig::poll_interval`. This bounds how long the owner can be
inside a read before observing a safety request.
