use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(feature = "waveshare-ugv")]
use std::io::Write;

#[cfg(feature = "waveshare-ugv")]
use anyhow::Context;
use anyhow::{anyhow, Result};
use parking_lot::{Mutex, RwLock};
#[cfg(feature = "waveshare-ugv")]
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::{sync::broadcast, time};
use tracing::{debug, warn};

use crate::{
    config::{HarnessConfig, Profile},
    types::{
        BatteryStatus, CameraStatus, Capabilities, CaptureResult, DriveOutcome, Health,
        OdometryStatus, RawFrameStatus, SensorSnapshot, SpeedMode, TelemetryFrame,
    },
};

trait RobotDriver: Send + Sync {
    fn drive(&self, left: f64, right: f64) -> Result<()>;

    fn stop(&self) -> Result<()> {
        self.drive(0.0, 0.0)
    }
}

#[derive(Debug)]
struct SimDriver;

impl RobotDriver for SimDriver {
    fn drive(&self, left: f64, right: f64) -> Result<()> {
        debug!(left, right, "sim drive");
        Ok(())
    }
}

#[cfg(feature = "waveshare-ugv")]
struct WaveshareUgvDriver {
    writer: Mutex<Box<dyn serialport::SerialPort>>,
    drive_invert: bool,
    drive_swap: bool,
}

#[cfg(feature = "waveshare-ugv")]
impl WaveshareUgvDriver {
    fn open(config: &HarnessConfig) -> Result<Self> {
        let port = serialport::new(&config.serial_port, config.serial_baud)
            .timeout(Duration::from_millis(200))
            .open()
            .with_context(|| {
                format!(
                    "open Waveshare UGV serial port {} @ {}",
                    config.serial_port, config.serial_baud
                )
            })?;
        Ok(Self {
            writer: Mutex::new(port),
            drive_invert: config.drive_invert,
            drive_swap: config.drive_swap,
        })
    }
}

#[cfg(feature = "waveshare-ugv")]
impl RobotDriver for WaveshareUgvDriver {
    fn drive(&self, left: f64, right: f64) -> Result<()> {
        let (mut left, mut right) = if self.drive_swap {
            (right, left)
        } else {
            (left, right)
        };
        if self.drive_invert {
            left = -left;
            right = -right;
        }
        let line = json!({"T": 1, "L": left, "R": right}).to_string() + "\n";
        let mut writer = self.writer.lock();
        writer
            .write_all(line.as_bytes())
            .context("write Waveshare UGV drive command")?;
        writer
            .flush()
            .context("flush Waveshare UGV drive command")?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct PilotSession {
    expires_at: Instant,
    speed_mode: SpeedMode,
}

#[derive(Debug, Clone)]
struct CommandState {
    left_cmd: f64,
    right_cmd: f64,
    last_cmd_at: Option<Instant>,
    active_session_id: Option<String>,
    speed_mode: SpeedMode,
    estop: bool,
    stopped_by_deadman: bool,
    soft_odometry_limited: bool,
}

impl Default for CommandState {
    fn default() -> Self {
        Self {
            left_cmd: 0.0,
            right_cmd: 0.0,
            last_cmd_at: None,
            active_session_id: None,
            speed_mode: SpeedMode::default(),
            estop: false,
            stopped_by_deadman: false,
            soft_odometry_limited: false,
        }
    }
}

#[derive(Debug, Clone)]
struct RawTelemetry {
    battery_v: Option<f64>,
    odometry_left: Option<f64>,
    odometry_right: Option<f64>,
    source: String,
    last_raw_frame_ms: Option<u128>,
}

impl RawTelemetry {
    fn sim() -> Self {
        Self {
            battery_v: Some(12.3),
            odometry_left: Some(0.0),
            odometry_right: Some(0.0),
            source: "sim".to_string(),
            last_raw_frame_ms: Some(now_ms()),
        }
    }

    fn physical() -> Self {
        Self {
            battery_v: None,
            odometry_left: None,
            odometry_right: None,
            source: "waveshare-ugv".to_string(),
            last_raw_frame_ms: None,
        }
    }
}

#[derive(Clone)]
pub struct Harness {
    config: HarnessConfig,
    started_at: Instant,
    driver: Arc<dyn RobotDriver>,
    command: Arc<Mutex<CommandState>>,
    sessions: Arc<Mutex<HashMap<String, PilotSession>>>,
    raw: Arc<RwLock<RawTelemetry>>,
    telemetry_tx: broadcast::Sender<TelemetryFrame>,
}

impl Harness {
    pub fn new(config: HarnessConfig) -> Result<Self> {
        config.validate()?;

        let driver: Arc<dyn RobotDriver> = match config.profile {
            Profile::Sim => Arc::new(SimDriver),
            Profile::WaveshareUgv => open_physical_driver(&config)?,
        };

        let raw = match config.profile {
            Profile::Sim => RawTelemetry::sim(),
            Profile::WaveshareUgv => RawTelemetry::physical(),
        };

        let (telemetry_tx, _) = broadcast::channel(128);
        let harness = Self {
            config,
            started_at: Instant::now(),
            driver,
            command: Arc::new(Mutex::new(CommandState::default())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            raw: Arc::new(RwLock::new(raw)),
            telemetry_tx,
        };
        harness.spawn_deadman();
        harness.spawn_telemetry_loop();
        Ok(harness)
    }

    pub fn config(&self) -> &HarnessConfig {
        &self.config
    }

    pub fn subscribe_telemetry(&self) -> broadcast::Receiver<TelemetryFrame> {
        self.telemetry_tx.subscribe()
    }

    pub fn health(&self) -> Health {
        let command = self.command.lock().clone();
        Health {
            ok: true,
            role: self.config.role.clone(),
            profile: self.config.profile.as_str().to_string(),
            uptime_ms: self.started_at.elapsed().as_millis(),
            estop: command.estop,
            deadman_ok: !command.stopped_by_deadman,
            physical_actuation_enabled: self.config.allow_physical_actuation
                || std::env::var("LEASH_ALLOW_PHYSICAL_ACTUATION")
                    .ok()
                    .as_deref()
                    == Some("1"),
        }
    }

    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            ok: true,
            role: self.config.role.clone(),
            profile: self.config.profile.as_str().to_string(),
            physical: self.config.profile.is_physical(),
            endpoints: vec![
                "GET /health".to_string(),
                "GET /capabilities".to_string(),
                "GET /telemetry".to_string(),
                "GET /sensors".to_string(),
                "GET /camera/status".to_string(),
                "POST /pilot/authorize".to_string(),
                "POST /drive".to_string(),
                "POST /motors/stop".to_string(),
                "POST /estop".to_string(),
                "POST /estop/reset".to_string(),
                "WS /ws/telemetry".to_string(),
            ],
            mcp_tools: vec![
                "health".to_string(),
                "capabilities".to_string(),
                "observe".to_string(),
                "invoke_capability".to_string(),
                "stop".to_string(),
                "estop".to_string(),
                "capture".to_string(),
            ],
            speed_modes: vec![SpeedMode::Low, SpeedMode::Medium, SpeedMode::High],
        }
    }

    pub fn telemetry(&self) -> TelemetryFrame {
        let now = now_ms();
        let command = self.command.lock().clone();
        let raw = self.raw.read().clone();
        let sensors = sensor_snapshot(&raw);
        TelemetryFrame {
            ts_ms: now,
            robot: self.config.role.clone(),
            profile: self.config.profile.as_str().to_string(),
            battery_v: raw.battery_v,
            left_cmd: command.left_cmd,
            right_cmd: command.right_cmd,
            odometry_left: raw.odometry_left,
            odometry_right: raw.odometry_right,
            session_id: command.active_session_id,
            deadman_ok: !command.stopped_by_deadman,
            estop: command.estop,
            stopped_by_deadman: command.stopped_by_deadman,
            soft_odometry_limited: command.soft_odometry_limited,
            soft_odometry_limit_m: self.config.soft_odometry_limit_m,
            speed_mode: command.speed_mode,
            max_speed: command.speed_mode.cap(),
            sensors,
            source: raw.source,
        }
    }

    pub fn authorize(&self, token: String, ttl_secs: u64, speed_mode: SpeedMode) -> Result<()> {
        if token.trim().is_empty() {
            return Err(anyhow!("token cannot be empty"));
        }
        self.sessions.lock().insert(
            token,
            PilotSession {
                expires_at: Instant::now() + Duration::from_secs(ttl_secs.max(1)),
                speed_mode,
            },
        );
        Ok(())
    }

    pub fn set_speed_mode(&self, token: Option<&str>, speed_mode: SpeedMode) -> Result<()> {
        self.validate_session(token)?;
        self.command.lock().speed_mode = speed_mode;
        Ok(())
    }

    pub fn drive(
        &self,
        token: Option<&str>,
        left: f64,
        right: f64,
        speed_mode: Option<SpeedMode>,
    ) -> Result<DriveOutcome> {
        let session = self.validate_session(token)?;
        let speed_mode = speed_mode.or(session.map(|session| session.speed_mode));
        if let Some(speed_mode) = speed_mode {
            self.command.lock().speed_mode = speed_mode;
        }

        let mut command = self.command.lock();
        if command.estop {
            return Err(anyhow!("estop is latched; call estop/reset before driving"));
        }

        let max_speed = command.speed_mode.cap();
        let mut left = clamp(left, -max_speed, max_speed);
        let mut right = clamp(right, -max_speed, max_speed);
        command.soft_odometry_limited = self.soft_odometry_limit_reached(left, right);
        if command.soft_odometry_limited {
            left = 0.0;
            right = 0.0;
        }

        self.driver.drive(left, right)?;
        command.left_cmd = left;
        command.right_cmd = right;
        command.last_cmd_at = Some(Instant::now());
        command.active_session_id = token.map(ToOwned::to_owned);
        command.stopped_by_deadman = false;
        drop(command);

        self.advance_sim_odometry(left, right);

        let command = self.command.lock().clone();
        Ok(DriveOutcome {
            ok: true,
            left,
            right,
            speed_mode: command.speed_mode,
            max_speed,
            stopped_by_deadman: command.stopped_by_deadman,
            soft_odometry_limited: command.soft_odometry_limited,
        })
    }

    pub fn stop(&self) -> Result<DriveOutcome> {
        self.driver.stop()?;
        let mut command = self.command.lock();
        command.left_cmd = 0.0;
        command.right_cmd = 0.0;
        command.last_cmd_at = Some(Instant::now());
        command.stopped_by_deadman = false;
        Ok(DriveOutcome {
            ok: true,
            left: 0.0,
            right: 0.0,
            speed_mode: command.speed_mode,
            max_speed: command.speed_mode.cap(),
            stopped_by_deadman: false,
            soft_odometry_limited: command.soft_odometry_limited,
        })
    }

    pub fn estop(&self) -> Result<()> {
        self.driver.stop()?;
        let mut command = self.command.lock();
        command.left_cmd = 0.0;
        command.right_cmd = 0.0;
        command.estop = true;
        command.stopped_by_deadman = false;
        Ok(())
    }

    pub fn reset_estop(&self) {
        let mut command = self.command.lock();
        command.estop = false;
        command.stopped_by_deadman = false;
    }

    pub fn capture(&self) -> CaptureResult {
        let frame = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="320" height="240"><rect width="320" height="240" fill="#101820"/><text x="18" y="120" fill="#f6f1d1" font-family="monospace" font-size="18">leash {}</text></svg>"##,
            self.config.role
        );
        let mut hasher = Sha256::new();
        hasher.update(frame.as_bytes());
        CaptureResult {
            ok: true,
            source: format!("{}-capture", self.config.profile.as_str()),
            content_type: "image/svg+xml".to_string(),
            byte_len: frame.len(),
            captured_at_ms: now_ms(),
            sha256: format!("{:x}", hasher.finalize()),
        }
    }

    fn validate_session(&self, token: Option<&str>) -> Result<Option<PilotSession>> {
        if self.config.allow_untokened_drive && token.is_none() {
            return Ok(None);
        }
        let token = token.ok_or_else(|| anyhow!("missing pilot token"))?;
        let mut sessions = self.sessions.lock();
        let Some(session) = sessions.get(token).cloned() else {
            return Err(anyhow!("invalid pilot token"));
        };
        if Instant::now() > session.expires_at {
            sessions.remove(token);
            return Err(anyhow!("expired pilot token"));
        }
        Ok(Some(session))
    }

    fn soft_odometry_limit_reached(&self, left: f64, right: f64) -> bool {
        if self.config.soft_odometry_limit_m <= 0.0 || left <= 0.0 && right <= 0.0 {
            return false;
        }
        let raw = self.raw.read();
        let Some(left_m) = raw.odometry_left else {
            return false;
        };
        let Some(right_m) = raw.odometry_right else {
            return false;
        };
        ((left_m + right_m) / 2.0).abs() >= self.config.soft_odometry_limit_m
    }

    fn advance_sim_odometry(&self, left: f64, right: f64) {
        if self.config.profile != Profile::Sim {
            return;
        }
        let mut raw = self.raw.write();
        raw.odometry_left = Some(round3(raw.odometry_left.unwrap_or_default() + left * 0.03));
        raw.odometry_right = Some(round3(
            raw.odometry_right.unwrap_or_default() + right * 0.03,
        ));
        raw.last_raw_frame_ms = Some(now_ms());
    }

    fn spawn_deadman(&self) {
        let harness = self.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_millis(50));
            loop {
                interval.tick().await;
                let should_stop = {
                    let command = harness.command.lock();
                    if command.left_cmd == 0.0 && command.right_cmd == 0.0 {
                        false
                    } else {
                        command.last_cmd_at.is_some_and(|at| {
                            at.elapsed().as_millis() > harness.config.deadman_ms as u128
                        })
                    }
                };
                if should_stop {
                    if let Err(err) = harness.driver.stop() {
                        warn!(?err, "deadman stop failed");
                    }
                    let mut command = harness.command.lock();
                    command.left_cmd = 0.0;
                    command.right_cmd = 0.0;
                    command.stopped_by_deadman = true;
                }
            }
        });
    }

    fn spawn_telemetry_loop(&self) {
        let harness = self.clone();
        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_millis(50));
            loop {
                interval.tick().await;
                let _ = harness.telemetry_tx.send(harness.telemetry());
            }
        });
    }
}

fn open_physical_driver(config: &HarnessConfig) -> Result<Arc<dyn RobotDriver>> {
    match config.profile {
        Profile::Sim => Ok(Arc::new(SimDriver)),
        Profile::WaveshareUgv => {
            #[cfg(feature = "waveshare-ugv")]
            {
                Ok(Arc::new(WaveshareUgvDriver::open(config)?))
            }
            #[cfg(not(feature = "waveshare-ugv"))]
            {
                let _ = config;
                Err(anyhow!(
                    "profile 'waveshare-ugv' requires building with --features waveshare-ugv"
                ))
            }
        }
    }
}

fn sensor_snapshot(raw: &RawTelemetry) -> SensorSnapshot {
    SensorSnapshot {
        battery: BatteryStatus {
            status: if raw.battery_v.is_some() {
                "available"
            } else {
                "unavailable"
            }
            .to_string(),
            voltage_v: raw.battery_v,
        },
        odometry: OdometryStatus {
            status: if raw.odometry_left.is_some() || raw.odometry_right.is_some() {
                "available"
            } else {
                "unavailable"
            }
            .to_string(),
            left_m: raw.odometry_left,
            right_m: raw.odometry_right,
        },
        camera: CameraStatus {
            status: "simulated".to_string(),
            health: "healthy".to_string(),
            stream_url: None,
            snapshot_url: None,
        },
        raw_frame: RawFrameStatus {
            status: if raw.last_raw_frame_ms.is_some() {
                "available"
            } else {
                "missing"
            }
            .to_string(),
            source: raw.source.clone(),
            last_ms: raw.last_raw_frame_ms,
        },
    }
}

fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.max(min).min(max)
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after unix epoch")
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sim_harness_drives_and_deadman_stops() {
        let config = HarnessConfig {
            deadman_ms: 20,
            ..HarnessConfig::default()
        };
        let harness = Harness::new(config).unwrap();

        let outcome = harness.drive(None, 1.0, 1.0, Some(SpeedMode::Low)).unwrap();
        assert_eq!(outcome.left, SpeedMode::Low.cap());
        assert_eq!(outcome.right, SpeedMode::Low.cap());

        time::sleep(Duration::from_millis(80)).await;
        let telemetry = harness.telemetry();
        assert_eq!(telemetry.left_cmd, 0.0);
        assert_eq!(telemetry.right_cmd, 0.0);
        assert!(telemetry.stopped_by_deadman);
    }

    #[test]
    fn physical_profile_requires_explicit_gate() {
        let config = HarnessConfig {
            profile: Profile::WaveshareUgv,
            ..HarnessConfig::default()
        };
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("LEASH_ALLOW_PHYSICAL_ACTUATION"));
    }

    #[tokio::test]
    async fn capture_is_deterministic_for_role() {
        let harness = Harness::new(HarnessConfig::default()).unwrap();
        assert_eq!(harness.capture().sha256, harness.capture().sha256);
    }
}
