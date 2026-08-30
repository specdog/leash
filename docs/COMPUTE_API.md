# Asynchronous Compute API

Leash exposes a bounded, authenticated job API for advisory compute workloads.
The API is generic: clients submit versioned jobs and receive typed evidence;
no client product is part of the Leash runtime or schema.

## Authority boundary

Compute results are advisory. They cannot authorize or refresh motor output.
CPU collision checks, command deadlines, pilot ownership, stop, E-stop, and the
physical adapter remain the final safety authority.

## Authentication

Set `LEASH_COMPUTE_TOKEN_FILE` to a private file containing one bearer token.
Every compute route, including capabilities and the event stream, requires:

```text
Authorization: Bearer <token>
```

The API fails closed with `503 Service Unavailable` when the token file is not
configured or cannot be read. Tokens are not accepted in request bodies or
URLs.

## Routes

| Method | Route | Purpose |
| --- | --- | --- |
| `GET` | `/compute/capabilities` | Limits, job types, backend gate, and authority statement |
| `POST` | `/compute/jobs` | Submit an idempotent asynchronous job |
| `GET` | `/compute/jobs/:job_id` | Read lifecycle state and the terminal result |
| `POST` | `/compute/jobs/:job_id/cancel` | Cancel queued or executing work |
| `GET` | `/events/compute` | Sequenced server-sent lifecycle events |

Requests and results are limited to 1 MiB. The registry retains at most 64
jobs, executes at most two concurrently, accepts deadlines up to 120 seconds,
and never selects more than 32 scans or 20,000 raw points.

## Spatial-window job

The initial job type is `spatial_window`. The server selects recent range scans
already paired with wheel-integrated poses in the local `odom` frame. Clients
do not upload arbitrary scan arrays.

```json
{
  "schema_version": "leash.compute-job.v1",
  "job_id": "mission-42-spatial-1",
  "idempotency_key": "mission-42:spatial:1",
  "job_type": "spatial_window",
  "priority": "interactive",
  "timeout_ms": 2000,
  "source": {
    "scan_count": 32,
    "max_age_ms": 10000
  }
}
```

`job_id` may be omitted for server assignment. Repeating an identical request
with the same idempotency key returns the existing job. Reusing the key with a
different request returns `409 Conflict`.

Lifecycle values are `queued`, `running`, `completed`, `failed`, and
`cancelled`. A completed record embeds `leash.spatial-evidence.v1`, including:

- exact source producer epoch, sequence, scan timestamp, pose timestamp, and
  frame IDs;
- valid points as flat `[x, y, ...]` coordinates in `odom`;
- requested and authoritative backend;
- shadow comparison count, CUDA qualification, fallback reason, input size,
  and elapsed time.

## CPU and CUDA selection

Workloads below 10,000 points remain on CPU. Larger windows can use CUDA only
when the runtime selected CUDA and 16 CPU-authoritative shadow comparisons have
matched within `1e-4` meters. Any mismatch or CUDA failure disqualifies the
accelerator for this workload and returns CPU evidence with an explicit
fallback receipt.

The temporal CUDA transform uses the single process-owned `leash-cuda`
executor and precompiled `sm_87`/`compute_87` artifact. It does not create a
second CUDA context.

## Events and restart behavior

Each SSE event uses `leash.compute-event.v1` and contains a producer epoch,
monotonic sequence, timestamp, job ID, and lifecycle state. A new process gets
a new producer epoch. Clients must reconcile terminal state with the job GET
route after reconnecting; an SSE stream is notification, not durable storage.
