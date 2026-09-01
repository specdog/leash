# Geek on a Leash — Rust Tuesdays speaker notes

## 01

This is the Rust Tuesdays version: substantially more source code, fewer broad architecture slides, and a line-by-line tour of the language mechanisms that make Leash safe enough to sit near motors.

[Sources]
- https://github.com/specdog/leash
- https://www.waveshare.com/product/ai/robots/ugv-rover-pt-jetson-orin-ai-kit.htm

## 02

The core problem is authority, not intelligence. Several producers may propose motion, but only one narrow boundary may authorize it. Rust lets us represent that authority in types rather than conventions.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/README.md

## 03

Four verbs define the boundary. The rest of this talk asks how Rust preserves that order across generics, traits, threads, drivers, serialization, ROS2, and CUDA.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/README.md

## 04

Pinkie is the concrete forcing function: Jetson compute, motor controller, LiDAR, camera, and an actual chassis. Every language decision we discuss has a physical consequence at this boundary.

[Sources]
- https://www.waveshare.com/product/ai/robots/ugv-rover-pt-jetson-orin-ai-kit.htm

## 05

The workspace structure is not packaging trivia. Core defines the legal vocabulary. Runtime owns authority. Adapters and accelerators depend inward, so they cannot redefine the rules without creating a visible dependency violation.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/Cargo.toml
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/Cargo.toml
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/Cargo.toml

## 06

Rust is not valuable here because it is fashionable or merely fast. It is valuable because validation, ownership, and lifecycle rules can be made structural and checked before the robot is powered.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/drive.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/supervisor.rs

## 07

Start with unsafe policy. Runtime and hardware adapters forbid it. CUDA denies it at the crate root, then opens one explicitly named module. That makes the unsafe surface searchable and reviewable.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/lib.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-waveshare/src/lib.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/src/lib.rs

## 08

The newtype is the first important move. NormalizedDrive is as cheap as f64 at runtime, but its private tuple field means external code must call the validating constructor. After construction, the rest of the system can treat range and finiteness as already proven.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/drive.rs

## 09

Derives encode semantics. Copy says this value has no unique resource identity. Debug makes evidence legible. PartialEq makes transition tests precise. The module may construct STOP directly because it owns the private invariant.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/drive.rs

## 10

This is a good use of macro_rules: generate repetitive, inspectable domain types while preserving nominal type separation. The macro removes boilerplate without erasing the unit distinction.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/units.rs

## 11

Sequence uses a standard-library niche type rather than a comment saying zero is reserved. new converts Option from NonZeroU64 into a domain Result. next composes checked arithmetic with and_then so zero and overflow stay explicit.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/time.rs

## 12

A u64 timestamp is easy to misuse. The newtype names the clock domain, and every operation is checked. A caller must handle overflow or reversed time explicitly instead of receiving a wrapped value.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/time.rs

## 13

This small generic method shows ownership doing useful work. Consuming self allows the envelope fields to move directly. FnOnce is the least restrictive correct callback bound because the transform is invoked exactly once.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/time.rs

## 14

Candidate is generic over the command payload, but identity and timing are shared. The constructor remains a plain data constructor: possession of Candidate never implies permission to actuate.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/drive.rs

## 15

Authorized is a typestate boundary. The runtime can require this type at the motor port. Adapters can inspect it, but only the defining module can create it. That changes authorization from a boolean convention into possession of an unforgeable value.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/drive.rs

## 16

Look at the function signature first. It consumes Candidate<C> and either returns Authorized<C> or a typed denial. The body is an audit trail: safety state, temporal direction, deadline, evidence sequence, then construction of the capability.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/drive.rs

## 17

Compile-fail doctests protect absence: raw floats must not enter the drive command, and downstream code must not forge authorization. Runtime tests cannot prove those APIs are impossible; compiler tests can.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/drive.rs

## 18

Map, Odom, Base, and Sensor are uninhabited marker types. PhantomData tells the compiler that Frame logically depends on Tag even though no Tag value exists at runtime. The result is zero-cost coordinate-frame separation.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/frame.rs

## 19

Robotics code is full of structurally identical coordinates with incompatible meaning. A phantom frame tag makes the missing transform visible at the call site. The explicit conversion becomes a reviewable operation instead of an assumption.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-core/src/frame.rs

## 20

ControllerIo is a trait alias pattern on stable Rust. The blanket implementation means serial ports, test doubles, and future transports participate automatically if they satisfy Read, Write, and Send.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-waveshare/src/lib.rs

## 21

The factory itself is object-safe and the closure blanket implementation keeps call sites light. FnMut is deliberate: opening a connection may update retry counters or consume mutable configuration.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-waveshare/src/lib.rs

## 22

Associated types are better than extra generic parameters here because each concrete port has one canonical acknowledgement and error type. The signature also makes the typestate boundary unavoidable: raw DifferentialDrive is not accepted.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/supervisor.rs

## 23

The supervisor stores acknowledgement state, so A belongs on the struct. The concrete port P is only needed to spawn the owner thread. The equality bound proves that whatever P emits is exactly the A the shared state can hold.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/supervisor.rs

## 24

Leash uses both dispatch models. The supervisor control loop remains generic and monomorphized. The transport is selected at runtime behind Box dyn ControllerIo. The useful question is where substitution happens and who owns the value.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/supervisor.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-waveshare/src/lib.rs

## 25

The owner thread receives port by move, so the caller cannot keep using it. Shared observation crosses the boundary through Arc. The closure is wrapped in catch_unwind so a panic becomes explicit supervisor state rather than silent loss of the actuator owner.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/supervisor.rs

## 26

RAII matters beyond memory. When the supervisor owner is dropped, it signals shutdown, wakes the worker, takes the JoinHandle out of its Option, and joins exactly once. Lifecycle behavior is co-located with lifecycle ownership.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/supervisor.rs

## 27

Boundedness is a correctness property for a real-time-ish control loop. Notice the ownership-aware result types: a failed send returns the value, and drop-oldest reports what was replaced. Nothing disappears implicitly.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/lane.rs

## 28

The control path chooses an overload policy explicitly. RejectNewest preserves older work. DropOldest preserves freshness. The caller sees which happened through the result type, so overload can enter evidence instead of becoming hidden latency.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/lane.rs

## 29

Stop and E-stop do not compete with normal proposals for bounded-lane space. Each kind has an atomic request count. fetch_update publishes a monotonic sequence and rejects exhaustion instead of wrapping.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/safety.rs

## 30

Priority is visible in control flow. E-stop is loaded before stop and returns immediately. Each counter is compared with its own seen watermark, preserving request counts without allocating a queue.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/safety.rs

## 31

Some data should queue; high-rate observations often should not. The latest slot models that directly. Option replace and take express displacement and consumption without sentinel values or cloning.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/src/latest.rs

## 32

The gateway does not pass loosely typed JSON inward. A tagged enum defines the allowed command set. Unknown fields fail closed, and serde errors are mapped into a domain error before any command reaches runtime.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-gateway/src/lib.rs

## 33

This is idiomatic error plumbing with a safety benefit: failures stay categorized. map_err adapts errors at subsystem boundaries, while the question-mark operator exits immediately and keeps the success path easy to audit.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-gateway/src/lib.rs

## 34

let-else is ideal when the interesting path requires a value. front borrows the oldest ticket without removing it. Only an available acknowledgement causes pop_front, so a pending ticket retains its exact queue position.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-waveshare/src/lib.rs

## 35

The adapter is where abstract runtime contracts become controller messages. It chooses CommandAck and PortError, accepts only an Authorized drive, and preserves the typed acknowledgement contract back into runtime.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-waveshare/src/lib.rs

## 36

The ROS2 crate carries the phantom-frame discipline to path proposals and navigation goals. A transform names both endpoints in its type. The adapter can translate Nav2 intent, but it still cannot produce motor authority directly.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-ros2/src/lib.rs

## 37

Before discussing a kernel launch, establish where the machine code comes from. The CUDA source is compiled deliberately with nvcc into a fatbin containing native SM 8.7 code and compute 8.7 PTX. Normal production builds copy that checked artifact; include_bytes embeds it in the Rust binary. Rebuild is an explicit release operation, not a surprise at service startup. The artifact contract test checks source digest, fatbin digest, byte count, and exported symbols.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/src/lib.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/build.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/tests/artifact_contract.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/kernels/prebuilt/sm_87/manifest.json

## 38

This is the kernel model in its smallest useful form. The host launches a grid of blocks. Each thread derives a global index from blockIdx, blockDim, and threadIdx. LaunchConfig may round the thread count upward, so the first correctness rule is a bounds check. Consecutive threads write consecutive output elements, producing coalesced global writes. index divided by depth maps the expanded output back to its source occupancy cell.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/kernels/leash_kernels.cu
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/src/lib.rs

## 39

The unsafe block is narrow because everything Rust can prove happens first. Arithmetic is checked. Device allocations are grown before views are borrowed. The input view is shared and the output view is mutable, so Rust still prevents overlapping host-side access. unsafe remains necessary because the compiler cannot inspect the external kernel ABI, prove device pointer validity, or know that the launch will respect those lengths. That is the exact proof obligation under review.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/src/device.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/src/lib.rs

## 40

Each valid ray independently decides whether it belongs to the collision sector. The angular delta is wrapped through atan2 of sine and cosine so the sector works across the negative-pi to positive-pi seam. CUDA lacks a direct portable atomic minimum for f32 in this target contract, so Leash first proves the range is finite and non-negative, reinterprets its bits as u32, and uses atomicMin. IEEE-754 bit ordering matches numeric ordering only under that non-negative constraint. Contention is acceptable for this small two-value reduction, but the output remains advisory to the CPU safety path.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/kernels/leash_kernels.cu
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/src/lib.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/src/gate.rs

## 41

This kernel is both inference and bounded online adaptation. Each thread owns one scalar lane across state, weight, bias, lower input, and top-down input, so there is no inter-thread dependency in the update. Separate arrays produce coalesced access for a warp. The kernel computes the prediction, two error terms, corrected activation, weight update, and bias update before writing back. Clamps are part of the numerical contract and match the Rust CPU oracle. A second variant adds three atomic reductions for prediction error, activation mean, and RMS while keeping the full resident state on device.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/kernels/leash_kernels.cu
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/src/lib.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/src/device.rs

## 42

Do not time only the device function. These numbers include the executor queue, host-to-device transfer, kernel launch, synchronization, and readback. Voxel projection is dramatically slower on CUDA end to end. Large lidar and the combined spatial path cross the break-even point. Large camera normalization wins only when a GPU consumer can keep the tensor on device. Resident cognition still loses at the measured sizes. This evidence is encoded back into the Rust workload decision rather than replaced by a blanket GPU preference.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/evidence/jetson-orin-nx-rv2-13-20260829.json
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/src/gate.rs

## 43

The gate has three explicit modes. In Shadow, the CPU result is returned to the caller while CUDA runs the same owned ComputeJob. compare_results walks the typed ComputeResult variants and records maximum absolute and relative error. Sixteen matching samples are required for each eligible workload; the recorded gate probe performed forty-eight randomized comparisons. Only then does mode become CUDA. A mismatch or CUDA failure sets mode back to CPU and degraded true. Context loss, launch error, timeout, and worker panic were injected; every case returned CPU authority within the overall 100 millisecond compute deadline.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/src/gate.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/evidence/jetson-orin-nx-rv2-13-20260829.json

## 44

Types prevent classes of misuse, but the runtime still needs empirical proof. Replay runs the same scenario twice and checks exact transitions and a stable digest. Recorded Jetson evidence supplies latency, deadline, durability, and physical E-stop measurements.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-replay/src/lib.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-nomotion-20260829.json
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/evidence/jetson-orin-nx-evidence-20260829.json
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-physical-rollout-20260829.json

## 45

Qualia is intentionally outside the open-source Leash boundary. It may build missions, ontologies, and semantic evidence asynchronously. It proposes. Leash remains the small, fast, local authority path that decides whether a physical command may proceed.

[Sources]
- https://github.com/specdog/leash

## 46

The live demo is deliberately bounded. First run the observation-only preflight. Motion requires an active operator token and explicit approval. Perform one low-drive, short-duration pulse, then show the verified-zero acknowledgement. If any gate is red, play the recorded fallback. Do not improvise motion.

[Sources]
- https://github.com/specdog/leash

## 47

The closing idea is simple: when software crosses into physical authority, make the rule visible in the type system. The QR opens the editable deck package and the source is at specdog/leash.

[Sources]
- https://github.com/specdog/leash
