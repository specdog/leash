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

This is the unsafe island opened on slide seven. Checked multiplication, integer conversion, capacity checks, and typed device slices precede the cudarc kernel launch. CUDA accelerates compute and shadow evaluation; the CPU control kernel remains final motion authority.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/src/lib.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/src/device.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-cuda/README.md

## 38

Types prevent classes of misuse, but the runtime still needs empirical proof. Replay runs the same scenario twice and checks exact transitions and a stable digest. Recorded Jetson evidence supplies latency, deadline, durability, and physical E-stop measurements.

[Sources]
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-replay/src/lib.rs
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-nomotion-20260829.json
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/evidence/jetson-orin-nx-evidence-20260829.json
- https://github.com/specdog/leash/blob/566bc569b24bf5f392291b142469282fcdfac2b3/crates/leash-runtime/evidence/jetson-orin-nx-rv2-16-physical-rollout-20260829.json

## 39

Qualia is intentionally outside the open-source Leash boundary. It may build missions, ontologies, and semantic evidence asynchronously. It proposes. Leash remains the small, fast, local authority path that decides whether a physical command may proceed.

[Sources]
- https://github.com/specdog/leash

## 40

The live demo is deliberately bounded. First run the observation-only preflight. Motion requires an active operator token and explicit approval. Perform one low-drive, short-duration pulse, then show the verified-zero acknowledgement. If any gate is red, play the recorded fallback. Do not improvise motion.

[Sources]
- https://github.com/specdog/leash

## 41

The closing idea is simple: when software crosses into physical authority, make the rule visible in the type system. The QR opens the editable deck package and the source is at specdog/leash.

[Sources]
- https://github.com/specdog/leash
