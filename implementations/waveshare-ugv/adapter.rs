use std::{
    env,
    io::{BufRead, BufReader, ErrorKind, Write},
    sync::Arc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::{
    adapter::{waveshare_drive_values, GimbalAdapter, MobileBaseAdapter},
    config::HarnessConfig,
    runtime::RobotDriver,
    types::{ImuSample, ImuStatus, PlanarRangeScan, RangeScanStatus, SensorDataStatus, Vector3Si},
};

const LD06_HEADER: u8 = 0x54;
const LD06_VERSION_LENGTH: u8 = 0x2c;
const LD06_PACKET_LEN: usize = 47;
const LD06_POINTS_PER_PACKET: usize = 12;
const STANDARD_GRAVITY_MPS2: f64 = 9.80665;

pub(crate) const WAVESHARE_SOURCE: &str = "waveshare-ugv";
pub(crate) const LD06_SOURCE: &str = "waveshare-ugv-ld06";

pub(crate) struct WaveshareUgvDriver {
    writer: Mutex<Box<dyn serialport::SerialPort>>,
    drive_invert: bool,
    drive_swap: bool,
}

impl WaveshareUgvDriver {
    pub(crate) fn open(config: &HarnessConfig) -> Result<Self> {
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

    fn write_json(&self, payload: Value, context: &'static str) -> Result<()> {
        let line = payload.to_string() + "\n";
        let mut writer = self.writer.lock();
        writer.write_all(line.as_bytes()).context(context)?;
        writer.flush().context(context)?;
        Ok(())
    }
}

impl RobotDriver for WaveshareUgvDriver {
    fn telemetry_reader(&self) -> Result<Option<Box<dyn serialport::SerialPort>>> {
        let writer = self.writer.lock();
        writer
            .try_clone()
            .map(Some)
            .context("clone Waveshare UGV serial port for telemetry")
    }

    fn enable_telemetry(&self) -> Result<()> {
        self.write_json(
            json!({"T": 142, "cmd": 100}),
            "set Waveshare UGV telemetry interval",
        )?;
        self.write_json(
            json!({"T": 131, "cmd": 1}),
            "enable Waveshare UGV telemetry flow",
        )?;
        self.request_telemetry()
    }

    fn request_telemetry(&self) -> Result<()> {
        self.write_json(json!({"T": 130}), "request Waveshare UGV base telemetry")
    }
}

impl MobileBaseAdapter for WaveshareUgvDriver {
    fn drive(&self, left: f64, right: f64) -> Result<()> {
        let (left, right) = waveshare_drive_values(left, right, self.drive_invert, self.drive_swap);
        self.write_json(
            json!({"T": 1, "L": left, "R": right}),
            "write Waveshare UGV drive command",
        )
    }
}

impl GimbalAdapter for WaveshareUgvDriver {
    fn aim_camera(&self, pan_deg: f64, tilt_deg: f64, speed: u32, accel: u32) -> Result<()> {
        self.write_json(
            json!({
                "T": 133,
                "X": pan_deg,
                "Y": tilt_deg,
                "SPD": speed,
                "ACC": accel
            }),
            "write Waveshare UGV camera gimbal command",
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WaveshareSensorConfig {
    pub(crate) lidar: Option<Ld06Config>,
    pub(crate) imu: ImuConfig,
}

impl WaveshareSensorConfig {
    pub(crate) fn from_env() -> Result<Self> {
        Self::from_lookup(|key| env::var(key).ok())
    }

    pub(crate) fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Result<Self> {
        let lidar_device = text(&mut lookup, "LEASH_UGV_LIDAR_DEVICE", "");
        let lidar = if lidar_device.is_empty() {
            None
        } else {
            Some(Ld06Config {
                device: lidar_device,
                baud: number(&mut lookup, "LEASH_UGV_LIDAR_BAUD", 230_400_u32)?,
                frame_id: text(&mut lookup, "LEASH_UGV_LIDAR_FRAME_ID", "base_scan"),
                range_min_m: number(&mut lookup, "LEASH_UGV_LIDAR_RANGE_MIN_M", 0.02_f64)?,
                range_max_m: number(&mut lookup, "LEASH_UGV_LIDAR_RANGE_MAX_M", 12.0_f64)?,
                bins: number(&mut lookup, "LEASH_UGV_LIDAR_BINS", 360_usize)?,
                min_intensity: number(&mut lookup, "LEASH_UGV_LIDAR_MIN_INTENSITY", 0_u8)?,
                yaw_offset_deg: number(&mut lookup, "LEASH_UGV_LIDAR_YAW_OFFSET_DEG", 180.0_f64)?,
                clockwise: boolean(&mut lookup, "LEASH_UGV_LIDAR_CLOCKWISE", false)?,
                body_masks: parse_body_masks(&text(
                    &mut lookup,
                    "LEASH_UGV_LIDAR_BODY_MASKS_DEG",
                    "",
                ))?,
                stale_after_ms: number(&mut lookup, "LEASH_UGV_LIDAR_STALE_MS", 500_u64)?,
                collision_threshold_m: number(
                    &mut lookup,
                    "LEASH_UGV_COLLISION_THRESHOLD_M",
                    0.25_f64,
                )?,
            })
        };
        let imu = ImuConfig {
            frame_id: text(&mut lookup, "LEASH_UGV_IMU_FRAME_ID", "base_link"),
            accel_lsb_per_g: number(&mut lookup, "LEASH_UGV_IMU_ACCEL_LSB_PER_G", 8192.0_f64)?,
            gyro_dps_per_lsb: number(&mut lookup, "LEASH_UGV_IMU_GYRO_DPS_PER_LSB", 0.0164_f64)?,
            axis_map: AxisMap::parse(&text(&mut lookup, "LEASH_UGV_IMU_AXIS_MAP", "x,y,z"))?,
            stale_after_ms: number(&mut lookup, "LEASH_UGV_IMU_STALE_MS", 500_u64)?,
        };
        let config = Self { lidar, imu };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.imu.frame_id.trim().is_empty() {
            return Err(anyhow!("LEASH_UGV_IMU_FRAME_ID cannot be empty"));
        }
        if !self.imu.accel_lsb_per_g.is_finite() || self.imu.accel_lsb_per_g <= 0.0 {
            return Err(anyhow!("LEASH_UGV_IMU_ACCEL_LSB_PER_G must be positive"));
        }
        if !self.imu.gyro_dps_per_lsb.is_finite() || self.imu.gyro_dps_per_lsb <= 0.0 {
            return Err(anyhow!("LEASH_UGV_IMU_GYRO_DPS_PER_LSB must be positive"));
        }
        if self.imu.stale_after_ms == 0 {
            return Err(anyhow!("LEASH_UGV_IMU_STALE_MS must be positive"));
        }
        if let Some(lidar) = &self.lidar {
            lidar.validate()?;
        }
        Ok(())
    }

    pub(crate) fn lidar_is_configured(&self) -> bool {
        self.lidar.is_some()
    }

    pub(crate) fn lidar_stale_after_ms(&self) -> Option<u64> {
        self.lidar.as_ref().map(|config| config.stale_after_ms)
    }

    pub(crate) fn collision_threshold_m(&self) -> Option<f64> {
        self.lidar
            .as_ref()
            .map(|config| config.collision_threshold_m)
    }
}

fn text(lookup: &mut impl FnMut(&str) -> Option<String>, key: &str, default: &str) -> String {
    lookup(key)
        .map(|value| value.trim().to_string())
        .unwrap_or_else(|| default.to_string())
}

fn number<T>(lookup: &mut impl FnMut(&str) -> Option<String>, key: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let Some(value) = lookup(key) else {
        return Ok(default);
    };
    value
        .trim()
        .parse::<T>()
        .map_err(|error| anyhow!("invalid {key}: {error}"))
}

fn boolean(
    lookup: &mut impl FnMut(&str) -> Option<String>,
    key: &str,
    default: bool,
) -> Result<bool> {
    let Some(value) = lookup(key) else {
        return Ok(default);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(anyhow!("invalid {key}: expected boolean")),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Ld06Config {
    device: String,
    baud: u32,
    frame_id: String,
    range_min_m: f64,
    range_max_m: f64,
    bins: usize,
    min_intensity: u8,
    yaw_offset_deg: f64,
    clockwise: bool,
    body_masks: Vec<AngleMask>,
    pub(crate) stale_after_ms: u64,
    pub(crate) collision_threshold_m: f64,
}

impl Ld06Config {
    fn validate(&self) -> Result<()> {
        if self.device.trim().is_empty() {
            return Err(anyhow!("LEASH_UGV_LIDAR_DEVICE cannot be empty"));
        }
        if self.baud == 0 {
            return Err(anyhow!("LEASH_UGV_LIDAR_BAUD must be positive"));
        }
        if self.frame_id.trim().is_empty() {
            return Err(anyhow!("LEASH_UGV_LIDAR_FRAME_ID cannot be empty"));
        }
        if !self.range_min_m.is_finite()
            || !self.range_max_m.is_finite()
            || self.range_min_m < 0.0
            || self.range_max_m <= self.range_min_m
        {
            return Err(anyhow!("invalid LD06 range bounds"));
        }
        if !(12..=4096).contains(&self.bins) {
            return Err(anyhow!("LEASH_UGV_LIDAR_BINS must be between 12 and 4096"));
        }
        if !self.yaw_offset_deg.is_finite() {
            return Err(anyhow!("LEASH_UGV_LIDAR_YAW_OFFSET_DEG must be finite"));
        }
        if self.stale_after_ms == 0 {
            return Err(anyhow!("LEASH_UGV_LIDAR_STALE_MS must be positive"));
        }
        if !self.collision_threshold_m.is_finite()
            || self.collision_threshold_m < self.range_min_m
            || self.collision_threshold_m > self.range_max_m
        {
            return Err(anyhow!("invalid LEASH_UGV_COLLISION_THRESHOLD_M"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct AngleMask {
    start_deg: f64,
    end_deg: f64,
}

impl AngleMask {
    fn contains(self, angle_deg: f64) -> bool {
        let start = normalize_degrees(self.start_deg);
        let end = normalize_degrees(self.end_deg);
        let angle = normalize_degrees(angle_deg);
        if start <= end {
            (start..=end).contains(&angle)
        } else {
            angle >= start || angle <= end
        }
    }
}

fn parse_body_masks(value: &str) -> Result<Vec<AngleMask>> {
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|mask| {
            let (start, end) = mask
                .trim()
                .split_once(':')
                .ok_or_else(|| anyhow!("invalid LEASH_UGV_LIDAR_BODY_MASKS_DEG entry '{mask}'"))?;
            let start_deg = start
                .trim()
                .parse::<f64>()
                .with_context(|| format!("invalid lidar body-mask start angle in '{mask}'"))?;
            let end_deg = end
                .trim()
                .parse::<f64>()
                .with_context(|| format!("invalid lidar body-mask end angle in '{mask}'"))?;
            if !start_deg.is_finite() || !end_deg.is_finite() {
                return Err(anyhow!("lidar body-mask angles must be finite"));
            }
            Ok(AngleMask { start_deg, end_deg })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ImuConfig {
    frame_id: String,
    accel_lsb_per_g: f64,
    gyro_dps_per_lsb: f64,
    axis_map: AxisMap,
    pub(crate) stale_after_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AxisMap([SignedAxis; 3]);

impl AxisMap {
    fn parse(value: &str) -> Result<Self> {
        let axes = value
            .split(',')
            .map(|axis| SignedAxis::parse(axis.trim()))
            .collect::<Result<Vec<_>>>()?;
        if axes.len() != 3 {
            return Err(anyhow!(
                "LEASH_UGV_IMU_AXIS_MAP must contain exactly three signed axes"
            ));
        }
        let mut used = [false; 3];
        for axis in &axes {
            if used[axis.index] {
                return Err(anyhow!(
                    "LEASH_UGV_IMU_AXIS_MAP must use each source axis once"
                ));
            }
            used[axis.index] = true;
        }
        Ok(Self([axes[0], axes[1], axes[2]]))
    }

    fn apply(self, input: [f64; 3]) -> Vector3Si {
        let value = |axis: SignedAxis| input[axis.index] * f64::from(axis.sign);
        Vector3Si {
            x: value(self.0[0]),
            y: value(self.0[1]),
            z: value(self.0[2]),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SignedAxis {
    index: usize,
    sign: i8,
}

impl SignedAxis {
    fn parse(value: &str) -> Result<Self> {
        let (sign, axis) = match value.as_bytes().first() {
            Some(b'-') => (-1, &value[1..]),
            Some(b'+') => (1, &value[1..]),
            _ => (1, value),
        };
        let index = match axis.to_ascii_lowercase().as_str() {
            "x" => 0,
            "y" => 1,
            "z" => 2,
            _ => return Err(anyhow!("invalid signed IMU axis '{value}'")),
        };
        Ok(Self { index, sign })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BaseTelemetryUpdate {
    pub(crate) battery_v: Option<f64>,
    pub(crate) odometry_left_m: Option<f64>,
    pub(crate) odometry_right_m: Option<f64>,
    pub(crate) imu: Option<ImuStatus>,
    pub(crate) raw: Value,
}

pub(crate) fn read_base_telemetry_loop(
    port: Box<dyn serialport::SerialPort>,
    config: ImuConfig,
    publish: Arc<dyn Fn(BaseTelemetryUpdate) + Send + Sync>,
    publish_status: Arc<dyn Fn(ImuStatus) + Send + Sync>,
) {
    let mut reader = BufReader::new(port);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => continue,
            Ok(_) => match parse_base_line(&line) {
                Ok(Some(frame)) => {
                    if let Some(update) = decode_base_frame(frame, &config, now_ms()) {
                        publish(update);
                    }
                }
                Ok(None) => {}
                Err(error) => publish_status(sensor_error_status(
                    SensorDataStatus::Malformed,
                    WAVESHARE_SOURCE,
                    error,
                )),
            },
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
                ) => {}
            Err(_) => {
                publish_status(sensor_error_status(
                    SensorDataStatus::Disconnected,
                    WAVESHARE_SOURCE,
                    "base telemetry disconnected",
                ));
                thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

fn parse_base_line(line: &str) -> Result<Option<Value>, &'static str> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(Some(value));
    }
    let Some(start) = trimmed.find('{') else {
        return Ok(None);
    };
    let Some(end) = trimmed.rfind('}') else {
        return Err("malformed base telemetry JSON");
    };
    if end <= start {
        return Err("malformed base telemetry JSON");
    }
    serde_json::from_str::<Value>(&trimmed[start..=end])
        .map(Some)
        .map_err(|_| "malformed base telemetry JSON")
}

pub(crate) fn decode_base_frame(
    frame: Value,
    config: &ImuConfig,
    received_ms: u128,
) -> Option<BaseTelemetryUpdate> {
    if !is_base_feedback(&frame) {
        return None;
    }
    let imu = decode_imu(&frame, config, received_ms);
    Some(BaseTelemetryUpdate {
        battery_v: voltage(&frame),
        odometry_left_m: odometry_m(&frame, "odl", &["odometry_left", "left_m"]),
        odometry_right_m: odometry_m(&frame, "odr", &["odometry_right", "right_m"]),
        imu,
        raw: frame,
    })
}

fn is_base_feedback(frame: &Value) -> bool {
    frame
        .get("T")
        .and_then(json_number)
        .is_some_and(|kind| (kind - 1001.0).abs() < f64::EPSILON)
        || frame.get("v").is_some()
        || frame.get("odl").is_some()
        || frame.get("odr").is_some()
}

fn voltage(frame: &Value) -> Option<f64> {
    const DIRECT_KEYS: &[&str] = &[
        "battery_v",
        "voltage_v",
        "loadVoltage_V",
        "load_voltage_v",
        "busVoltage_V",
        "bus_voltage_v",
        "vbat",
        "VBAT",
        "voltage",
    ];
    for key in DIRECT_KEYS {
        if let Some(value) = frame.get(*key).and_then(json_number) {
            return normalize_voltage(value);
        }
    }
    frame
        .get("v")
        .and_then(json_number)
        .and_then(normalize_voltage)
}

fn normalize_voltage(value: f64) -> Option<f64> {
    let voltage = if value > 100.0 { value / 100.0 } else { value };
    (voltage > 3.0 && voltage < 30.0).then_some(voltage)
}

fn odometry_m(frame: &Value, centimeters_key: &str, meter_keys: &[&str]) -> Option<f64> {
    for key in meter_keys {
        if let Some(value) = frame.get(*key).and_then(json_number) {
            return Some(value);
        }
    }
    frame
        .get(centimeters_key)
        .and_then(json_number)
        .map(|centimeters| centimeters / 100.0)
}

fn decode_imu(frame: &Value, config: &ImuConfig, received_ms: u128) -> Option<ImuStatus> {
    const KEYS: [&str; 6] = ["ax", "ay", "az", "gx", "gy", "gz"];
    let present = KEYS.iter().filter(|key| frame.get(**key).is_some()).count();
    if present == 0 {
        return None;
    }
    if present != KEYS.len() {
        return Some(sensor_error_status(
            SensorDataStatus::Malformed,
            WAVESHARE_SOURCE,
            "base IMU frame is missing axes",
        ));
    }
    let values = KEYS
        .iter()
        .map(|key| frame.get(*key).and_then(json_number))
        .collect::<Option<Vec<_>>>();
    let Some(values) = values else {
        return Some(sensor_error_status(
            SensorDataStatus::Malformed,
            WAVESHARE_SOURCE,
            "base IMU frame contains non-numeric axes",
        ));
    };
    let acceleration_scale = STANDARD_GRAVITY_MPS2 / config.accel_lsb_per_g;
    let gyro_scale = config.gyro_dps_per_lsb.to_radians();
    let sample = ImuSample {
        ts_ms: received_ms,
        frame_id: config.frame_id.clone(),
        linear_acceleration_mps2: config.axis_map.apply([
            values[0] * acceleration_scale,
            values[1] * acceleration_scale,
            values[2] * acceleration_scale,
        ]),
        angular_velocity_radps: config.axis_map.apply([
            values[3] * gyro_scale,
            values[4] * gyro_scale,
            values[5] * gyro_scale,
        ]),
        orientation_xyzw: None,
    };
    let status = ImuStatus {
        status: SensorDataStatus::Available,
        source: WAVESHARE_SOURCE.to_string(),
        last_ms: Some(received_ms),
        sample: Some(sample),
        error: None,
    };
    if let Err(error) = status.validate() {
        return Some(sensor_error_status(
            SensorDataStatus::Malformed,
            WAVESHARE_SOURCE,
            format!("base IMU contract error: {error}"),
        ));
    }
    Some(status)
}

fn json_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

pub(crate) fn spawn_ld06_reader(
    config: Ld06Config,
    publish: Arc<dyn Fn(RangeScanStatus) + Send + Sync>,
) {
    thread::spawn(move || loop {
        match serialport::new(&config.device, config.baud)
            .timeout(Duration::from_millis(100))
            .open()
        {
            Ok(mut port) => read_ld06_port(&mut *port, &config, &publish),
            Err(_) => {
                publish(sensor_error_status(
                    SensorDataStatus::Disconnected,
                    LD06_SOURCE,
                    "lidar device unavailable",
                ));
                thread::sleep(Duration::from_millis(500));
            }
        }
    });
}

fn read_ld06_port(
    port: &mut dyn serialport::SerialPort,
    config: &Ld06Config,
    publish: &Arc<dyn Fn(RangeScanStatus) + Send + Sync>,
) {
    let mut parser = Ld06Parser::new(config.clone());
    let mut bytes = [0_u8; 1024];
    let mut last_status: Option<RangeScanStatus> = None;
    let mut stale_published = false;
    loop {
        match port.read(&mut bytes) {
            Ok(0) => {}
            Ok(count) => {
                for event in parser.push(&bytes[..count], now_ms()) {
                    match event {
                        Ld06Event::Scan(status) => {
                            let status = *status;
                            last_status = Some(status.clone());
                            stale_published = false;
                            publish(status);
                        }
                        Ld06Event::Malformed(error) => publish(sensor_error_status(
                            SensorDataStatus::Malformed,
                            LD06_SOURCE,
                            error,
                        )),
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
                ) => {}
            Err(_) => {
                let mut status = last_status.unwrap_or_else(|| RangeScanStatus {
                    source: LD06_SOURCE.to_string(),
                    ..RangeScanStatus::default()
                });
                status.status = SensorDataStatus::Disconnected;
                status.error = Some("lidar disconnected".to_string());
                publish(status);
                return;
            }
        }
        if !stale_published {
            if let Some(status) = last_status.as_ref() {
                if status.last_ms.is_some_and(|last_ms| {
                    now_ms().saturating_sub(last_ms) > u128::from(config.stale_after_ms)
                }) {
                    let mut stale = status.clone();
                    stale.status = SensorDataStatus::Stale;
                    stale.error = None;
                    publish(stale);
                    stale_published = true;
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Ld06Event {
    Scan(Box<RangeScanStatus>),
    Malformed(&'static str),
}

#[derive(Debug)]
struct Ld06Parser {
    buffer: Vec<u8>,
    assembler: Ld06Assembler,
}

impl Ld06Parser {
    fn new(config: Ld06Config) -> Self {
        Self {
            assembler: Ld06Assembler::new(config.clone()),
            buffer: Vec::with_capacity(LD06_PACKET_LEN * 3),
        }
    }

    fn push(&mut self, bytes: &[u8], received_ms: u128) -> Vec<Ld06Event> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        loop {
            let Some(header) = self.buffer.iter().position(|byte| *byte == LD06_HEADER) else {
                self.buffer.clear();
                break;
            };
            if header > 0 {
                self.buffer.drain(..header);
            }
            if self.buffer.len() < 2 {
                break;
            }
            if self.buffer[1] != LD06_VERSION_LENGTH {
                self.buffer.drain(..1);
                continue;
            }
            if self.buffer.len() < LD06_PACKET_LEN {
                break;
            }
            let packet = self.buffer[..LD06_PACKET_LEN].to_vec();
            if crc8(&packet[..LD06_PACKET_LEN - 1]) != packet[LD06_PACKET_LEN - 1] {
                events.push(Ld06Event::Malformed("invalid LD06 packet checksum"));
                self.buffer.drain(..1);
                continue;
            }
            self.buffer.drain(..LD06_PACKET_LEN);
            match Ld06Packet::decode(&packet) {
                Some(packet) => {
                    if let Some(scan) = self.assembler.push(packet, received_ms) {
                        let status = RangeScanStatus {
                            status: SensorDataStatus::Available,
                            source: LD06_SOURCE.to_string(),
                            last_ms: Some(received_ms),
                            sample: Some(scan),
                            error: None,
                        };
                        if status.validate().is_ok() {
                            events.push(Ld06Event::Scan(Box::new(status)));
                        } else {
                            events.push(Ld06Event::Malformed(
                                "assembled LD06 scan failed validation",
                            ));
                        }
                    }
                }
                None => events.push(Ld06Event::Malformed("malformed LD06 packet")),
            }
        }
        if self.buffer.capacity() > LD06_PACKET_LEN * 8 {
            self.buffer.shrink_to(LD06_PACKET_LEN * 3);
        }
        events
    }
}

#[derive(Debug, Clone, Copy)]
struct Ld06Point {
    distance_mm: u16,
    intensity: u8,
}

#[derive(Debug, Clone)]
struct Ld06Packet {
    speed_deg_per_sec: u16,
    start_angle_deg: f64,
    end_angle_deg: f64,
    points: [Ld06Point; LD06_POINTS_PER_PACKET],
}

impl Ld06Packet {
    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != LD06_PACKET_LEN
            || bytes[0] != LD06_HEADER
            || bytes[1] != LD06_VERSION_LENGTH
        {
            return None;
        }
        let speed_deg_per_sec = u16::from_le_bytes([bytes[2], bytes[3]]);
        let start_angle_deg = f64::from(u16::from_le_bytes([bytes[4], bytes[5]])) / 100.0;
        let end_angle_deg = f64::from(u16::from_le_bytes([bytes[42], bytes[43]])) / 100.0;
        let points = std::array::from_fn(|index| {
            let offset = 6 + index * 3;
            Ld06Point {
                distance_mm: u16::from_le_bytes([bytes[offset], bytes[offset + 1]]),
                intensity: bytes[offset + 2],
            }
        });
        let span = positive_angle_delta(start_angle_deg, end_angle_deg);
        if speed_deg_per_sec == 0 || span > 45.0 {
            return None;
        }
        Some(Self {
            speed_deg_per_sec,
            start_angle_deg,
            end_angle_deg,
            points,
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ScanBin {
    range_m: Option<f64>,
    intensity: Option<f64>,
}

#[derive(Debug)]
struct Ld06Assembler {
    config: Ld06Config,
    bins: Vec<ScanBin>,
    last_start_angle_deg: Option<f64>,
    speed_sum: f64,
    packet_count: usize,
    saw_first_wrap: bool,
}

impl Ld06Assembler {
    fn new(config: Ld06Config) -> Self {
        Self {
            bins: vec![ScanBin::default(); config.bins],
            config,
            last_start_angle_deg: None,
            speed_sum: 0.0,
            packet_count: 0,
            saw_first_wrap: false,
        }
    }

    fn push(&mut self, packet: Ld06Packet, received_ms: u128) -> Option<PlanarRangeScan> {
        let wrapped = self.last_start_angle_deg.is_some_and(|previous| {
            previous > 300.0 && packet.start_angle_deg < 60.0
                || packet.start_angle_deg + 180.0 < previous
        });
        let completed = if wrapped {
            let scan = self.saw_first_wrap.then(|| self.finish(received_ms));
            self.saw_first_wrap = true;
            self.clear();
            scan
        } else {
            None
        };
        self.add_packet(&packet);
        self.last_start_angle_deg = Some(packet.start_angle_deg);
        completed
    }

    fn add_packet(&mut self, packet: &Ld06Packet) {
        let step = positive_angle_delta(packet.start_angle_deg, packet.end_angle_deg)
            / (LD06_POINTS_PER_PACKET - 1) as f64;
        for (index, point) in packet.points.iter().enumerate() {
            let raw_angle = normalize_degrees(packet.start_angle_deg + step * index as f64);
            let direction = if self.config.clockwise { -1.0 } else { 1.0 };
            let angle =
                normalize_signed_degrees(direction * raw_angle + self.config.yaw_offset_deg);
            if self
                .config
                .body_masks
                .iter()
                .any(|mask| mask.contains(angle))
            {
                continue;
            }
            let normalized = normalize_degrees(angle + 180.0);
            let bin_index = ((normalized / 360.0) * self.config.bins as f64).floor() as usize
                % self.config.bins;
            let range_m = f64::from(point.distance_mm) / 1000.0;
            if range_m < self.config.range_min_m
                || range_m > self.config.range_max_m
                || point.intensity < self.config.min_intensity
            {
                continue;
            }
            let bin = &mut self.bins[bin_index];
            if bin.range_m.is_none_or(|current| range_m < current) {
                bin.range_m = Some(range_m);
                bin.intensity = Some(f64::from(point.intensity));
            }
        }
        self.speed_sum += f64::from(packet.speed_deg_per_sec) / 360.0;
        self.packet_count += 1;
    }

    fn finish(&self, received_ms: u128) -> PlanarRangeScan {
        let increment = std::f64::consts::TAU / self.config.bins as f64;
        PlanarRangeScan {
            ts_ms: received_ms,
            frame_id: self.config.frame_id.clone(),
            angle_min_rad: -std::f64::consts::PI,
            angle_max_rad: std::f64::consts::PI - increment,
            angle_increment_rad: increment,
            range_min_m: self.config.range_min_m,
            range_max_m: self.config.range_max_m,
            ranges_m: self.bins.iter().map(|bin| bin.range_m).collect(),
            intensities: self.bins.iter().map(|bin| bin.intensity).collect(),
            scan_rate_hz: (self.packet_count > 0)
                .then_some(self.speed_sum / self.packet_count as f64),
        }
    }

    fn clear(&mut self) {
        self.bins.fill(ScanBin::default());
        self.speed_sum = 0.0;
        self.packet_count = 0;
    }
}

pub(crate) fn with_freshness(
    mut status: RangeScanStatus,
    now_ms: u128,
    stale_after_ms: u64,
) -> RangeScanStatus {
    if status.status == SensorDataStatus::Available
        && status
            .last_ms
            .is_some_and(|last_ms| now_ms.saturating_sub(last_ms) > u128::from(stale_after_ms))
    {
        status.status = SensorDataStatus::Stale;
    }
    status
}

pub(crate) fn imu_with_freshness(
    mut status: ImuStatus,
    now_ms: u128,
    stale_after_ms: u64,
) -> ImuStatus {
    if status.status == SensorDataStatus::Available
        && status
            .last_ms
            .is_some_and(|last_ms| now_ms.saturating_sub(last_ms) > u128::from(stale_after_ms))
    {
        status.status = SensorDataStatus::Stale;
    }
    status
}

pub(crate) fn scan_blocks_drive(
    status: &RangeScanStatus,
    threshold_m: f64,
    left: f64,
    right: f64,
) -> bool {
    if status.status != SensorDataStatus::Available
        || left.abs() <= f64::EPSILON && right.abs() <= f64::EPSILON
    {
        return false;
    }
    let Some(scan) = status.sample.as_ref() else {
        return false;
    };
    let close_at_any_bearing = || {
        scan.ranges_m
            .iter()
            .flatten()
            .any(|range| *range <= threshold_m)
    };
    let linear = (left + right) * 0.5;
    if linear.abs() <= f64::EPSILON {
        return close_at_any_bearing();
    }
    if !scan.angle_min_rad.is_finite() || !scan.angle_increment_rad.is_finite() {
        return close_at_any_bearing();
    }
    let travel_bearing = if linear > 0.0 {
        0.0
    } else {
        std::f64::consts::PI
    };
    let half_sector = std::f64::consts::FRAC_PI_3;
    scan.ranges_m.iter().enumerate().any(|(index, range)| {
        range.is_some_and(|range| {
            range <= threshold_m
                && wrapped_angle_delta(
                    scan.angle_min_rad + index as f64 * scan.angle_increment_rad,
                    travel_bearing,
                )
                .abs()
                    <= half_sector
        })
    })
}

fn wrapped_angle_delta(left: f64, right: f64) -> f64 {
    let mut delta = left - right;
    while delta > std::f64::consts::PI {
        delta -= std::f64::consts::TAU;
    }
    while delta < -std::f64::consts::PI {
        delta += std::f64::consts::TAU;
    }
    delta
}

fn sensor_error_status<T>(status: SensorDataStatus, source: &str, error: impl Into<String>) -> T
where
    T: SensorErrorStatus,
{
    T::error(status, source, error.into())
}

trait SensorErrorStatus {
    fn error(status: SensorDataStatus, source: &str, error: String) -> Self;
}

impl SensorErrorStatus for RangeScanStatus {
    fn error(status: SensorDataStatus, source: &str, error: String) -> Self {
        Self {
            status,
            source: source.to_string(),
            error: Some(error),
            ..Self::default()
        }
    }
}

impl SensorErrorStatus for ImuStatus {
    fn error(status: SensorDataStatus, source: &str, error: String) -> Self {
        Self {
            status,
            source: source.to_string(),
            error: Some(error),
            ..Self::default()
        }
    }
}

fn positive_angle_delta(start_deg: f64, end_deg: f64) -> f64 {
    normalize_degrees(end_deg - start_deg)
}

fn normalize_degrees(value: f64) -> f64 {
    value.rem_euclid(360.0)
}

fn normalize_signed_degrees(value: f64) -> f64 {
    (value + 180.0).rem_euclid(360.0) - 180.0
}

fn crc8(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0_u8, |crc, byte| {
        let mut value = crc ^ byte;
        for _ in 0..8 {
            value = if value & 0x80 != 0 {
                (value << 1) ^ 0x4d
            } else {
                value << 1
            };
        }
        value
    })
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn config() -> WaveshareSensorConfig {
        WaveshareSensorConfig::from_lookup(|key| {
            let values = HashMap::from([
                ("LEASH_UGV_LIDAR_DEVICE", "/dev/test-lidar"),
                ("LEASH_UGV_LIDAR_BINS", "36"),
                ("LEASH_UGV_LIDAR_YAW_OFFSET_DEG", "0"),
                ("LEASH_UGV_LIDAR_BODY_MASKS_DEG", "-10:10"),
                ("LEASH_UGV_IMU_AXIS_MAP", "y,-x,z"),
            ]);
            values.get(key).map(ToString::to_string)
        })
        .unwrap()
    }

    fn packet(start_deg: f64, end_deg: f64, distance_mm: u16, intensity: u8) -> Vec<u8> {
        let mut bytes = vec![0_u8; LD06_PACKET_LEN];
        bytes[0] = LD06_HEADER;
        bytes[1] = LD06_VERSION_LENGTH;
        bytes[2..4].copy_from_slice(&3600_u16.to_le_bytes());
        bytes[4..6].copy_from_slice(&((start_deg * 100.0).round() as u16).to_le_bytes());
        for index in 0..LD06_POINTS_PER_PACKET {
            let offset = 6 + index * 3;
            bytes[offset..offset + 2].copy_from_slice(&distance_mm.to_le_bytes());
            bytes[offset + 2] = intensity;
        }
        bytes[42..44].copy_from_slice(&((end_deg * 100.0).round() as u16).to_le_bytes());
        bytes[44..46].copy_from_slice(&42_u16.to_le_bytes());
        bytes[46] = crc8(&bytes[..46]);
        bytes
    }

    fn decode_hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn checked_in_scrubbed_fixture_decodes_with_stable_units_and_crc() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../examples/waveshare-ugv/sensor-fixture.json"
        ))
        .unwrap();
        let config = config();
        let update = decode_base_frame(fixture["base_frame"].clone(), &config.imu, 1000).unwrap();
        assert_eq!(update.battery_v, fixture["expected"]["battery_v"].as_f64());
        assert_eq!(
            update.odometry_left_m,
            fixture["expected"]["odometry_left_m"].as_f64()
        );

        let bytes = decode_hex(fixture["ld06_packet_hex"].as_str().unwrap());
        assert_eq!(crc8(&bytes[..46]), bytes[46]);
        let packet = Ld06Packet::decode(&bytes).unwrap();
        assert_eq!(
            f64::from(packet.speed_deg_per_sec) / 360.0,
            fixture["expected"]["packet_speed_hz"].as_f64().unwrap()
        );
        assert_eq!(packet.points.len(), LD06_POINTS_PER_PACKET);
        assert_eq!(packet.start_angle_deg, 0.0);
        assert_eq!(packet.end_angle_deg, 9.0);
    }

    #[test]
    fn implementation_config_parses_masks_transforms_and_calibration() {
        let config = config();
        let lidar = config.lidar.unwrap();
        assert_eq!(lidar.baud, 230_400);
        assert_eq!(lidar.bins, 36);
        assert!(lidar.body_masks[0].contains(0.0));
        assert!(!lidar.body_masks[0].contains(90.0));
        assert_eq!(
            config.imu.axis_map.apply([1.0, 2.0, 3.0]),
            Vector3Si {
                x: 2.0,
                y: -1.0,
                z: 3.0
            }
        );
    }

    #[test]
    fn implementation_config_rejects_invalid_values() {
        let error = WaveshareSensorConfig::from_lookup(|key| {
            (key == "LEASH_UGV_IMU_AXIS_MAP").then(|| "x,x,z".to_string())
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("use each source axis once"));

        let error = WaveshareSensorConfig::from_lookup(|key| match key {
            "LEASH_UGV_LIDAR_DEVICE" => Some("/dev/test".to_string()),
            "LEASH_UGV_COLLISION_THRESHOLD_M" => Some("20".to_string()),
            _ => None,
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("COLLISION_THRESHOLD"));
    }

    #[test]
    fn base_frame_converts_raw_imu_to_body_si_units() {
        let config = config();
        let update = decode_base_frame(
            json!({
                "T": 1001,
                "v": 1206,
                "odl": 5,
                "odr": 3,
                "ax": 8192,
                "ay": 0,
                "az": 0,
                "gx": 100,
                "gy": 0,
                "gz": 0
            }),
            &config.imu,
            123,
        )
        .unwrap();
        assert_eq!(update.battery_v, Some(12.06));
        assert_eq!(update.odometry_left_m, Some(0.05));
        let sample = update.imu.unwrap().sample.unwrap();
        assert!((sample.linear_acceleration_mps2.y + STANDARD_GRAVITY_MPS2).abs() < 1e-9);
        assert!((sample.angular_velocity_radps.y + (1.64_f64).to_radians()).abs() < 1e-9);
        assert_eq!(sample.ts_ms, 123);
        assert_eq!(sample.orientation_xyzw, None);
    }

    #[test]
    fn base_frame_marks_missing_and_invalid_imu_axes_malformed() {
        let config = config();
        let update =
            decode_base_frame(json!({"T": 1001, "ax": 1, "ay": 2}), &config.imu, 1).unwrap();
        assert_eq!(update.imu.unwrap().status, SensorDataStatus::Malformed);

        let update = decode_base_frame(
            json!({"T": 1001, "ax": "bad", "ay": 2, "az": 3, "gx": 4, "gy": 5, "gz": 6}),
            &config.imu,
            1,
        )
        .unwrap();
        assert_eq!(update.imu.unwrap().status, SensorDataStatus::Malformed);
    }

    #[test]
    fn ld06_parser_checks_crc_resynchronizes_and_emits_full_binned_scan() {
        let lidar = config().lidar.unwrap();
        let mut parser = Ld06Parser::new(lidar);
        let mut corrupt = packet(0.0, 9.0, 1000, 20);
        corrupt[10] ^= 0xff;
        let events = parser.push(&corrupt, 1);
        assert_eq!(
            events,
            vec![Ld06Event::Malformed("invalid LD06 packet checksum")]
        );

        let mut output = Vec::new();
        for revolution in 0..3 {
            for start in (0..360).step_by(10) {
                let bytes = packet(start as f64, start as f64 + 9.0, 1000, 20);
                for chunk in bytes.chunks(7) {
                    output.extend(parser.push(chunk, 100 + revolution * 100));
                }
            }
        }
        let scan = output
            .into_iter()
            .find_map(|event| match event {
                Ld06Event::Scan(status) => status.sample,
                Ld06Event::Malformed(_) => None,
            })
            .expect("complete scan");
        assert_eq!(scan.ranges_m.len(), 36);
        assert_eq!(scan.intensities.len(), 36);
        assert_eq!(scan.scan_rate_hz, Some(10.0));
        assert!(scan.ranges_m.iter().flatten().count() >= 30);
        assert!(
            scan.ranges_m[18].is_none(),
            "body mask must invalidate front bins"
        );
        scan.validate().unwrap();
    }

    #[test]
    fn lidar_calibration_filters_range_intensity_and_wraparound_body_masks() {
        let mut lidar = config().lidar.unwrap();
        lidar.min_intensity = 10;
        lidar.body_masks = parse_body_masks("170:-170").unwrap();
        let mut assembler = Ld06Assembler::new(lidar);
        let weak = Ld06Packet::decode(&packet(80.0, 89.0, 1000, 5)).unwrap();
        assembler.add_packet(&weak);
        assert!(assembler.bins.iter().all(|bin| bin.range_m.is_none()));

        let close = Ld06Packet::decode(&packet(80.0, 89.0, 1, 20)).unwrap();
        assembler.add_packet(&close);
        assert!(assembler.bins.iter().all(|bin| bin.range_m.is_none()));
    }

    #[test]
    fn freshness_and_collision_gate_degrade_without_panicking() {
        let mut scan = RangeScanStatus {
            status: SensorDataStatus::Available,
            source: LD06_SOURCE.to_string(),
            last_ms: Some(100),
            sample: Some(PlanarRangeScan {
                ts_ms: 100,
                frame_id: "base_scan".to_string(),
                angle_min_rad: -1.0,
                angle_max_rad: 1.0,
                angle_increment_rad: 1.0,
                range_min_m: 0.02,
                range_max_m: 12.0,
                ranges_m: vec![Some(1.0), None, Some(0.2)],
                intensities: vec![Some(1.0), None, Some(2.0)],
                scan_rate_hz: Some(10.0),
            }),
            error: None,
        };
        assert!(scan_blocks_drive(&scan, 0.25, 0.1, 0.1));
        scan = with_freshness(scan, 601, 500);
        assert_eq!(scan.status, SensorDataStatus::Stale);
        assert!(!scan_blocks_drive(&scan, 0.25, 0.1, 0.1));
    }

    #[test]
    fn collision_gate_checks_the_travel_sector_and_keeps_rotation_conservative() {
        let scan = RangeScanStatus {
            status: SensorDataStatus::Available,
            source: LD06_SOURCE.to_string(),
            last_ms: Some(100),
            sample: Some(PlanarRangeScan {
                ts_ms: 100,
                frame_id: "base_scan".to_string(),
                angle_min_rad: -std::f64::consts::PI,
                angle_max_rad: std::f64::consts::PI,
                angle_increment_rad: std::f64::consts::FRAC_PI_4,
                range_min_m: 0.02,
                range_max_m: 12.0,
                ranges_m: vec![
                    Some(0.2),
                    Some(1.0),
                    Some(1.0),
                    Some(1.0),
                    Some(1.0),
                    Some(1.0),
                    Some(1.0),
                    Some(1.0),
                    Some(0.2),
                ],
                intensities: vec![Some(1.0); 9],
                scan_rate_hz: Some(10.0),
            }),
            error: None,
        };
        assert!(!scan_blocks_drive(&scan, 0.25, 0.1, 0.1));
        assert!(scan_blocks_drive(&scan, 0.25, -0.1, -0.1));
        assert!(scan_blocks_drive(&scan, 0.25, -0.1, 0.1));
        assert!(!scan_blocks_drive(&scan, 0.25, 0.0, 0.0));
    }

    #[test]
    fn parser_buffer_stays_bounded_during_long_stationary_input() {
        let lidar = config().lidar.unwrap();
        let mut parser = Ld06Parser::new(lidar);
        let bytes = packet(10.0, 19.0, 1000, 20);
        for _ in 0..100_000 {
            parser.push(&bytes, 1);
        }
        assert!(parser.buffer.len() < LD06_PACKET_LEN);
        assert!(parser.buffer.capacity() <= LD06_PACKET_LEN * 8);
        assert_eq!(parser.assembler.bins.len(), 36);
    }
}
