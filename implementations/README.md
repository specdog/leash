# Implementations

This folder contains concrete robots built on the reusable Leash library. An
implementation may select devices, middleware, calibration, and deployment
policy. It must consume Leash's public contracts and must not move those private
or machine-specific choices into the core library.

```mermaid
flowchart TB
  library["Leash library\ntraits, messages, safety, replay"] --> ugv["Waveshare UGV implementation"]
  ugv --> devices["motor, camera, lidar, and IMU adapters"]
  ugv --> deploy["private deployment and rollback state"]
  ugv --> slam["localization and mapping adapter"]

  devices -. "never hard-coded in core" .-> private["device paths and calibration"]
  deploy -. "never committed" .-> private
```

## Implementations

- [`waveshare-ugv/`](waveshare-ugv/): the concrete Jetson/Waveshare UGV,
  including deployment, sensor, mapping, and supervised field-proof material.
