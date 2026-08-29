# leash-runtime

`leash-runtime` owns synchronization and scheduling around `leash-core`.
Domain transitions remain synchronous. This crate provides only bounded,
non-blocking orchestration primitives:

- an atomic safety mailbox that prioritizes e-stop and preserves request counts;
- bounded single-consumer lanes with reject-newest or drop-oldest behavior;
- a latest-value sensor slot that rejects non-increasing sample sequences;
- snapshots for depth, high-water marks, drops, and rejections.

Queue capacity and overflow policy are part of the API. No unbounded channel is
available from this crate.
