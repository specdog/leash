# Adapter contracts and second-UGV checklist

Leash keeps operator behavior and safety policy above small Rust adapter
contracts in `src/adapter.rs`:

- `MobileBaseAdapter` owns differential drive and fail-safe zero-speed stop.
- `GimbalAdapter` owns pan/tilt movement and fails closed by default.
- `CameraAdapter` produces capture and stream command plans with stable content
  types. The current `FfmpegV4l2CameraAdapter` preserves the existing HTTP and
  operator wire behavior.

The Waveshare implementation now implements the mobile-base and gimbal traits.
Its swap/invert transform is a separately tested pure function; its serial JSON
commands, telemetry requests, actuation gate, and policy behavior are unchanged.

## Adding a second UGV

1. Add an explicit Cargo feature and physical profile/stack metadata. Declare
   adapter category `mobile-base`, maturity, capabilities, and required gates.
2. Implement `MobileBaseAdapter::drive`. Confirm units, sign convention,
   clamping, left/right ordering, and that `stop()` writes zero to both sides.
3. Implement `GimbalAdapter` only when the bot has a verified gimbal. Otherwise
   use the fail-closed default and omit the capability from adapter metadata.
4. Implement `CameraAdapter` capture and stream plans for a new backend, or use
   `FfmpegV4l2CameraAdapter`. Preserve `image/jpeg` snapshots and the advertised
   multipart stream content type so the operator does not need bot-specific UI.
5. Add telemetry parsing behind the adapter boundary. Sanitize raw hardware
   errors and never emit device paths, commands, environment variables, or
   bearer tokens through public health/telemetry types.
6. Preserve both safety layers: compile-time feature opt-in and runtime
   `physical-actuation`. Physical motion must also pass token/approval policy;
   stop and e-stop must remain available when other commands are denied.
7. Add sim/replay fixtures first. Hardware discovery or a missing device must
   never make default builds, tests, schemas, or smokes require the bot.
8. Add adapter unit tests, feature-matrix coverage, schema proof, package proof,
   and a completed copy of `ADAPTER_SMOKE_TEMPLATE.md` before fleet use.

Do not commit real fleet IPs, credentials, serial numbers, bot names, or private
network diagrams. Store deployment-specific values in the private fleet config
or host environment.
