# Geek on a Leash — Speaker Notes

## 01

Welcome. This is a talk about a small piece of software with an intentionally narrow job: stand between an intelligent requester and physical motors. We will spend most of the hour inside Leash, then use Pinkie for a bounded live proof.

[Sources]
- https://github.com/specdog/leash
- https://www.waveshare.com/product/ai/robots/ugv-rover-pt-jetson-orin-ai-kit.htm

## 02

Start at the boundary. In an agentic robot, the planner, human, autonomy stack, and recovery code may all want motion. If authority is implicit, every integration becomes a safety argument. Leash makes the question explicit.

## 03

The metaphor is literal enough to be useful. Leash does not invent missions, recognize rooms, or decide what is interesting. It constrains the path from requested action to the physical body.

[Sources]
- https://github.com/specdog/leash

## 04

This is the canonical rule from the project README. I use it as a design filter: language models and planners produce candidates; only Leash can authorize a candidate for hardware execution.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/README.md

## 05

Pinkie is the current robot. The left image is a live camera snapshot from today; the right is the official Waveshare product image. The important point is not the brand. It is that real serial buses, stale sensors, batteries, and motor controllers turn vague intent into concrete failure modes.

[Sources]
- https://www.waveshare.com/product/ai/robots/ugv-rover-pt-jetson-orin-ai-kit.htm
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/implementations/waveshare-ugv/README.md

## 06

Follow a motion request downward. Protocol parsing cannot write motors. The registry resolves a capability. Policy validates and authorizes. The runtime selects one adapter. Only that adapter owns the hardware path.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/README.md
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/lib.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/implementations/waveshare-ugv/adapter.rs

## 07

The single-owner rule is operational, not philosophical. The runtime status reports the Waveshare controller owner. If another process owns the serial port, Leash cannot make a meaningful authority claim.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/lib.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/implementations/waveshare-ugv/adapter.rs

## 08

Leash exposes CLI, HTTP, and MCP-shaped access, but those are transports. The capability boundary is the stable idea. A caller submits a typed candidate and receives an authorization or a refusal—not direct hardware access.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/src/http.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/src/capability.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/src/runtime.rs

## 09

This is the architectural split. Intent is rich and nondeterministic. Authorization and the safety kernel are small and deterministic. The adapter translates to hardware. Evidence records the whole crossing so a later replay can explain what happened.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-evidence/src

## 10

Capabilities name the effect and its contract. Unknown verbs do not fall through to magic. Unavailable hardware is not silently simulated. Each call ends as authorized, rejected, or unavailable, and that outcome can be recorded.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/src/capability.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/src/types.rs

## 11

This is real Leash syntax. ControllerIo is a marker trait with three supertraits: Read, Write, and Send. The blanket implementation means any type satisfying those bounds automatically implements ControllerIo. There are no methods to fake and no inheritance tree. The Send bound is architectural: ownership of the I/O object can move into the controller-owner thread.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-waveshare/src/lib.rs

## 12

Leash’s CPU supervisor runs with a default ten-millisecond tick. It reads bounded inputs, makes a deterministic transition, writes an effect, and records evidence. The language-model loop is outside this deadline.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/safety_supervisor.rs

## 13

NormalizedDrive is a one-field tuple struct whose field is private. Callers cannot construct NormalizedDrive(2.0); they must use new, which rejects non-finite and out-of-range values and returns Result. After construction, the rest of the program can rely on the invariant instead of rechecking every float.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/drive.rs

## 14

Candidate<C> and Authorized<C> carry the same generic command type C, but they are different states. authorize consumes the candidate, checks the gate and deadline, allocates evidence identity, and returns Authorized<C>. Authorized’s fields are private, so a transport cannot assemble one with a struct literal. The compile-fail doctest proves that forgery does not compile.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/drive.rs

## 15

The supervisor has a bounded proposal channel and a separate priority safety path. Emergency stop is not allowed to wait behind ordinary motion requests. The CPU loop remains the final authority even when CUDA shadow computation is active.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/safety_supervisor.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/README.md

## 16

ActuationPort uses associated types rather than trait generics. Each implementation chooses one acknowledgement and error type, so the supervisor can be generic over the port without erasing its concrete contract. The Send plus static bounds allow the port to live in the supervisor thread. submit_drive takes mutable self and only Authorized<DifferentialDrive>, enforcing exclusive access and post-policy input at the signature.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/supervisor.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-waveshare/src/lib.rs

## 17

Today’s read-only sensor endpoint reported fresh LiDAR, camera, IMU, and odometry plus 83.3 percent battery. The design lesson is that a sensor value must carry freshness and health; stale but plausible values are dangerous.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/src/types.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/src/http.rs

## 18

The live runtime reported more than 4.8 million durable records. The point is not the number; it is the sequence. Replay should reconstruct the request, authorization, state transition, hardware write, telemetry, and result without asking an agent to narrate what it thinks happened.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-evidence/src
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src

## 19

The safety lifecycle is explicit. Motion is not a boolean. Arm, move, stop, verify zero, and fault are different states with defined exits. Timeouts, E-stop, stale sensors, and ownership loss all have an explicit route away from motion.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/safety_supervisor.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/drive.rs

## 20

In the physical rollout evidence, E-stop acknowledgement was measured at 37.622 milliseconds and the final state was verified zero. This is the distinction: sending a stop is not the success condition. Observing and recording zero is.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-physical-rollout-20260829.json

## 21

CUDA is active on Pinkie, but the live runtime reports CPU final authority. The CUDA design is shadow-first: compare results, measure performance, and fall back. Acceleration is not permission, and GPU availability is not a reason to widen the authority surface.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/README.md
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/evidence/jetson-orin-nx-rv2-13-20260829.json

## 22

Map and Odom are zero-variant marker enums. Frame<Tag> carries PhantomData, so the tag exists at compile time without runtime storage. Pose2<Map> and Pose2<Odom> are different types; the compile-fail doctest proves they cannot be exchanged silently. The ROS2 bridge converts Nav2 output into typed proposals, but Leash still authorizes the effect.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/frame.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-ros2/src/lib.rs

## 23

These are measured artifacts committed with Leash: 58,306 nanoseconds p99 transition latency, zero deadline misses at 100 hertz, 110,293 durable records per second in the evidence run, and 37.622 milliseconds physical E-stop acknowledgement. They are not promises for every machine; they are reproducible evidence from this Jetson.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-nomotion-20260829.json
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/evidence/jetson-orin-nx-evidence-20260829.json
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-physical-rollout-20260829.json

## 24

Leash is MIT licensed and modular. Core types, runtime policy, hardware adapters, evidence, and CUDA can evolve independently so long as they preserve the authority contract. The goal is not a universal robotics framework; it is a sharp boundary that can fit inside different stacks.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/LICENSE
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/Cargo.toml
- https://github.com/specdog/leash

## 25

For the demo, I am separating live proof from future direction. Today we verified ownership, CPU authority, fresh sensors, camera, CUDA availability, and evidence. Mapping is currently initializing and visual odometry is unavailable, so I will not claim autonomous mapping or SLAM lock.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/README.md

## 26

Qualia is intentionally outside Leash. It works on scene understanding, missions, ontologies, and longer-lived learning. Those asynchronous updates may improve future proposals, but they do not enter the ten-millisecond safety loop or gain motor authority.

[Sources]
- https://github.com/specdog/leash

## 27

This is the entire relationship in one slide. Hermes or Qualia can form a mission and candidate. The candidate includes bounded magnitude, duration, and context. Leash validates and either authorizes an effect or records a refusal. The mission system remains replaceable.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/src/types.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/src/capability.rs
- https://github.com/specdog/leash

## 28

The demo sequence is intentionally boring. First a read-only preflight. Then an explicit human approval and operator token. One low-speed pulse, no more than 0.10 normalized drive and no more than 500 milliseconds. Then verified stop and evidence. If any gate is red, I use the recorded fallback instead of improvising.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-physical-rollout-20260829.json

## 29

The takeaway is the rule: the planner proposes and the boundary decides. The QR points directly to this deck’s PDF and expires on the date shown. The repository is public and MIT licensed. Thank you—questions are welcome.

[Sources]
- https://github.com/specdog/leash
- https://docs.railway.com/storage-buckets
