//! Camera lighting driven by measured JPEG luminance. Commands use the existing
//! controller owner; a serial write never establishes physical lamp feedback.

use std::{collections::VecDeque, io::Read, path::PathBuf};

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DARK: f64 = 0.18;
const BRIGHT_RESET: f64 = 0.28;
const SUSTAIN_MS: u64 = 2_000;
const MAX_SAMPLE_GAP_MS: u64 = 1_500;
const COOLDOWN_MS: u64 = 10_000;
const AFTER_WINDOW_MS: u64 = 5_000;
const MEASUREMENT_INTERVAL_MS: u64 = 200;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LightMode {
    Off,
    Manual,
    Auto,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightRequest {
    pub mode: LightMode,
    pub camera_pwm: Option<u8>,
    pub chassis_pwm: Option<u8>,
    pub token: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LightWriteReceipt {
    pub controller_sequence: u64,
    pub write_completed_at_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LumaSample {
    pub request_started_ms: u64,
    pub received_ms: u64,
    pub width: u32,
    pub height: u32,
    pub mean_luma: f64,
    pub jpeg_sha256: String,
    pub capture_timestamp_known: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct LightEvent {
    pub sequence: u64,
    pub origin: &'static str,
    pub requested_at_ms: u64,
    pub chassis_pwm: u8,
    pub camera_pwm: u8,
    pub receipt: Option<LightWriteReceipt>,
    pub error: Option<String>,
    pub before: Option<LumaSample>,
    pub after: Option<LumaSample>,
    pub physical_feedback: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct LightStatus {
    pub schema_version: &'static str,
    pub enabled: bool,
    pub mode: LightMode,
    pub auto_on_latched: bool,
    pub configured_camera_pwm: u8,
    pub configured_chassis_pwm: u8,
    pub last_written_camera_pwm: Option<u8>,
    pub last_written_chassis_pwm: Option<u8>,
    pub physical_state_known: bool,
    pub physical_feedback: &'static str,
    pub dark_threshold: f64,
    pub bright_reset_threshold: f64,
    pub sustained_ms: u64,
    pub cooldown_ms: u64,
    pub measurement_interval_ms: u64,
    pub last_sample: Option<LumaSample>,
    pub last_error: Option<String>,
    pub events: Vec<LightEvent>,
}

pub(crate) struct Lighting {
    enabled: bool,
    mode: LightMode,
    auth_token: Option<String>,
    camera_pwm: u8,
    chassis_pwm: u8,
    last_written: Option<(u8, u8)>,
    latched: bool,
    dark_since: Option<u64>,
    dark_samples: usize,
    previous_sample_ms: Option<u64>,
    last_attempt_ms: Option<u64>,
    last_sample: Option<LumaSample>,
    last_error: Option<String>,
    events: VecDeque<LightEvent>,
    sequence: u64,
    measurement_in_progress: bool,
    last_measurement_started_ms: Option<u64>,
}

fn flag(name: &str) -> Result<bool> {
    match std::env::var(name) {
        Err(std::env::VarError::NotPresent) => Ok(false),
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "on" => Ok(true),
            "0" | "false" | "off" | "" => Ok(false),
            _ => bail!("{name} must be true or false"),
        },
        Err(_) => bail!("{name} is not valid text"),
    }
}

impl Lighting {
    pub(crate) fn from_env(physical_gate: bool) -> Result<Self> {
        let requested = flag("LEASH_ALLOW_CAMERA_LIGHTS")?;
        let auto = flag("LEASH_CAMERA_LIGHT_AUTO")?;
        if auto && (!requested || !physical_gate) {
            bail!("automatic camera lighting requires explicit light and physical-actuation gates on Waveshare");
        }
        let path = std::env::var_os("LEASH_LIGHTS_AUTH_TOKEN_FILE")
            .or_else(|| std::env::var_os("LEASH_OPERATOR_AUTH_TOKEN_FILE"));
        let token = if requested {
            path.map(|path| {
                let file = std::fs::File::open(PathBuf::from(path))
                    .map_err(|_| anyhow!("light authorization token file is unreadable"))?;
                let mut bytes = Vec::new();
                file.take(4097)
                    .read_to_end(&mut bytes)
                    .map_err(|_| anyhow!("light authorization token file is unreadable"))?;
                if bytes.len() > 4096 {
                    bail!("light authorization token file is too large");
                }
                let token = String::from_utf8(bytes)
                    .map_err(|_| anyhow!("light authorization token is not UTF-8"))?;
                let token = token.trim().to_owned();
                if token.is_empty() {
                    bail!("light authorization token is empty");
                }
                Ok(token)
            })
            .transpose()?
        } else {
            None
        };
        Ok(Self::new(requested && physical_gate, auto, token))
    }

    fn new(enabled: bool, auto: bool, auth_token: Option<String>) -> Self {
        Self {
            enabled,
            mode: if auto {
                LightMode::Auto
            } else {
                LightMode::Off
            },
            auth_token,
            camera_pwm: 180,
            chassis_pwm: 0,
            last_written: None,
            latched: false,
            dark_since: None,
            dark_samples: 0,
            previous_sample_ms: None,
            last_attempt_ms: None,
            last_sample: None,
            last_error: None,
            events: VecDeque::new(),
            sequence: 0,
            measurement_in_progress: false,
            last_measurement_started_ms: None,
        }
    }

    pub(crate) fn begin_measurement(&mut self, now: u64) -> bool {
        if !self.enabled
            || self.measurement_in_progress
            || self
                .last_measurement_started_ms
                .is_some_and(|previous| now.saturating_sub(previous) < MEASUREMENT_INTERVAL_MS)
        {
            return false;
        }
        self.measurement_in_progress = true;
        self.last_measurement_started_ms = Some(now);
        true
    }

    pub(crate) fn end_measurement(&mut self) {
        self.measurement_in_progress = false;
    }

    pub(crate) fn status(&self) -> LightStatus {
        LightStatus {
            schema_version: "leash.camera-lights.v1",
            enabled: self.enabled,
            mode: self.mode,
            auto_on_latched: self.latched,
            configured_camera_pwm: self.camera_pwm,
            configured_chassis_pwm: self.chassis_pwm,
            last_written_chassis_pwm: self.last_written.map(|value| value.0),
            last_written_camera_pwm: self.last_written.map(|value| value.1),
            physical_state_known: false,
            physical_feedback: "unknown",
            dark_threshold: DARK,
            bright_reset_threshold: BRIGHT_RESET,
            sustained_ms: SUSTAIN_MS,
            cooldown_ms: COOLDOWN_MS,
            measurement_interval_ms: MEASUREMENT_INTERVAL_MS,
            last_sample: self.last_sample.clone(),
            last_error: self.last_error.clone(),
            events: self.events.iter().cloned().collect(),
        }
    }

    pub(crate) fn configure(
        &mut self,
        request: LightRequest,
        now: u64,
        write: impl FnOnce(u8, u8) -> Result<LightWriteReceipt>,
    ) -> Result<LightStatus> {
        if !self.enabled {
            bail!("camera lighting is disabled by the deployment gate");
        }
        if !self
            .auth_token
            .as_deref()
            .zip(request.token.as_deref())
            .is_some_and(|(expected, supplied)| token_equal(expected, supplied))
        {
            bail!("camera lighting requires its configured operator token");
        }
        let camera = match request.mode {
            LightMode::Off => 0,
            LightMode::Manual => request
                .camera_pwm
                .ok_or_else(|| anyhow!("manual lighting requires camera_pwm"))?,
            LightMode::Auto => request.camera_pwm.unwrap_or(180),
        };
        if request.mode == LightMode::Auto && camera == 0 {
            bail!("automatic lighting requires a nonzero camera_pwm");
        }
        let chassis = request.chassis_pwm.unwrap_or(self.chassis_pwm);
        if request.mode == LightMode::Auto && chassis != self.chassis_pwm {
            bail!("set chassis_pwm in manual mode before arming camera auto mode");
        }
        self.mode = request.mode;
        self.camera_pwm = camera;
        self.chassis_pwm = chassis;
        self.latched =
            request.mode == LightMode::Auto && self.last_written.is_some_and(|(_, pwm)| pwm > 0);
        self.reset_darkness();
        // An authorized off/manual request disarms automatic writes even if its
        // serial write fails. The requested state and receipt remain distinct.
        if request.mode != LightMode::Auto {
            self.issue(chassis, camera, now, "operator", write)?;
        }
        Ok(self.status())
    }

    fn reset_darkness(&mut self) {
        self.dark_since = None;
        self.dark_samples = 0;
    }

    pub(crate) fn acquisition_failed(&mut self, error: String) {
        self.reset_darkness();
        self.previous_sample_ms = None;
        self.last_error = Some(error);
    }

    pub(crate) fn observe(
        &mut self,
        sample: LumaSample,
        now: u64,
        write: impl FnOnce(u8, u8) -> Result<LightWriteReceipt>,
    ) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if !sample.mean_luma.is_finite()
            || !(0.0..=1.0).contains(&sample.mean_luma)
            || sample.request_started_ms > sample.received_ms
            || sample.received_ms > now
            || now.saturating_sub(sample.request_started_ms) > 2_000
        {
            self.acquisition_failed("camera luminance is stale or invalid".into());
            return Ok(());
        }
        // Only the latest attempt can receive an after-image: an intervening
        // write, including one with a failed acknowledgement, breaks attribution.
        if let Some(event) = self.events.back_mut() {
            if let Some(receipt) = &event.receipt {
                if event.after.is_none()
                    && sample.request_started_ms
                        >= receipt.write_completed_at_ms.saturating_add(250)
                    && sample.received_ms
                        <= receipt
                            .write_completed_at_ms
                            .saturating_add(AFTER_WINDOW_MS)
                {
                    event.after = Some(sample.clone());
                }
            }
        }
        if self.previous_sample_ms.is_none_or(|previous| {
            sample.received_ms <= previous || sample.received_ms - previous > MAX_SAMPLE_GAP_MS
        }) {
            self.reset_darkness();
        }
        self.previous_sample_ms = Some(sample.received_ms);
        self.last_sample = Some(sample.clone());
        if self.mode != LightMode::Auto || self.latched {
            return Ok(());
        }
        if sample.mean_luma >= BRIGHT_RESET {
            self.reset_darkness();
            return Ok(());
        }
        if sample.mean_luma >= DARK {
            return Ok(());
        }
        let since = *self.dark_since.get_or_insert(sample.received_ms);
        self.dark_samples += 1;
        if self.dark_samples < 3
            || sample.received_ms.saturating_sub(since) < SUSTAIN_MS
            || self
                .last_attempt_ms
                .is_some_and(|previous| now.saturating_sub(previous) < COOLDOWN_MS)
        {
            return Ok(());
        }
        self.issue(
            self.chassis_pwm,
            self.camera_pwm,
            now,
            "auto-darkness",
            write,
        )?;
        self.latched = true;
        self.reset_darkness();
        Ok(())
    }

    fn issue(
        &mut self,
        chassis: u8,
        camera: u8,
        now: u64,
        origin: &'static str,
        write: impl FnOnce(u8, u8) -> Result<LightWriteReceipt>,
    ) -> Result<()> {
        self.last_attempt_ms = Some(now);
        self.sequence = self.sequence.saturating_add(1);
        let result = write(chassis, camera);
        let (receipt, error) = match result {
            Ok(receipt) => {
                self.last_written = Some((chassis, camera));
                (Some(receipt), None)
            }
            Err(error) => (None, Some(error.to_string())),
        };
        self.last_error = error.clone();
        if self.events.len() == 64 {
            self.events.pop_front();
        }
        self.events.push_back(LightEvent {
            sequence: self.sequence,
            origin,
            requested_at_ms: now,
            chassis_pwm: chassis,
            camera_pwm: camera,
            receipt,
            error: error.clone(),
            before: self
                .last_sample
                .clone()
                .filter(|sample| now.saturating_sub(sample.request_started_ms) <= 2_000),
            after: None,
            physical_feedback: "unknown",
        });
        if let Some(error) = error {
            return Err(anyhow!(error));
        }
        Ok(())
    }
}

fn token_equal(expected: &str, supplied: &str) -> bool {
    if expected.len() != supplied.len() {
        return false;
    }
    expected
        .bytes()
        .zip(supplied.bytes())
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

pub(crate) fn measure_jpeg(bytes: &[u8], started_ms: u64, received_ms: u64) -> Result<LumaSample> {
    if bytes.len() > 4 * 1024 * 1024 {
        bail!("camera JPEG exceeds the luminance measurement limit");
    }
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let pixels = reader.decode()?.to_luma8();
    if pixels.is_empty() {
        bail!("camera JPEG contains no luminance samples");
    }
    let sum: u64 = pixels.as_raw().iter().map(|value| u64::from(*value)).sum();
    Ok(LumaSample {
        request_started_ms: started_ms,
        received_ms,
        width: pixels.width(),
        height: pixels.height(),
        mean_luma: sum as f64 / (pixels.len() as f64 * 255.0),
        jpeg_sha256: format!("{:x}", Sha256::digest(bytes)),
        capture_timestamp_known: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(at: u64, luma: f64) -> LumaSample {
        LumaSample {
            request_started_ms: at - 10,
            received_ms: at,
            width: 640,
            height: 480,
            mean_luma: luma,
            jpeg_sha256: "test-observation".into(),
            capture_timestamp_known: false,
        }
    }
    fn receipt(at: u64) -> LightWriteReceipt {
        LightWriteReceipt {
            controller_sequence: 7,
            write_completed_at_ms: at,
        }
    }

    #[test]
    fn darkness_must_persist_and_light_remains_latched_after_scene_brightens() {
        let mut lights = Lighting::new(true, true, None);
        for at in [1000, 2000] {
            lights
                .observe(sample(at, 0.1), at, |_, _| panic!("too early"))
                .unwrap();
        }
        lights
            .observe(sample(3000, 0.1), 3000, |base, camera| {
                assert_eq!((base, camera), (0, 180));
                Ok(receipt(3000))
            })
            .unwrap();
        lights
            .observe(sample(4000, 0.8), 4000, |_, _| {
                panic!("lamp must not switch off on illuminated frame")
            })
            .unwrap();
        let status = lights.status();
        assert!(status.auto_on_latched);
        assert!(!status.physical_state_known);
        assert_eq!(status.events.len(), 1);
        assert_eq!(status.events[0].before.as_ref().unwrap().mean_luma, 0.1);
        assert_eq!(status.events[0].after.as_ref().unwrap().mean_luma, 0.8);
    }

    #[test]
    fn stale_gaps_and_bright_hysteresis_reset_pending_darkness() {
        let mut lights = Lighting::new(true, true, None);
        for (at, luma) in [
            (1000, 0.1),
            (2000, 0.1),
            (3000, 0.5),
            (4000, 0.1),
            (7000, 0.1),
            (8000, 0.1),
        ] {
            lights
                .observe(sample(at, luma), at, |_, _| panic!("not sustained"))
                .unwrap();
        }
        lights
            .observe(sample(9000, 0.1), 9000, |_, _| Ok(receipt(9000)))
            .unwrap();
        assert_eq!(lights.status().events.len(), 1);
    }

    #[test]
    fn failed_writes_remain_unknown_and_retries_obey_cooldown() {
        let mut lights = Lighting::new(true, true, None);
        for at in [1000, 2000] {
            lights
                .observe(sample(at, 0.1), at, |_, _| panic!())
                .unwrap();
        }
        assert!(lights
            .observe(sample(3000, 0.1), 3000, |_, _| Err(anyhow!("disconnected")))
            .is_err());
        for at in (4000..13000).step_by(1000) {
            lights
                .observe(sample(at, 0.1), at, |_, _| panic!("cooldown"))
                .unwrap();
        }
        assert_eq!(lights.status().last_written_camera_pwm, None);
        lights
            .observe(sample(13000, 0.1), 13000, |_, _| Ok(receipt(13000)))
            .unwrap();
        assert_eq!(lights.status().events.len(), 2);
    }

    #[test]
    fn configuration_requires_light_authority_and_off_disarms_auto() {
        let mut lights = Lighting::new(true, true, Some("test-only-light-authority".into()));
        let request = |token| LightRequest {
            mode: LightMode::Off,
            camera_pwm: None,
            chassis_pwm: None,
            token,
        };
        assert!(lights
            .configure(request(Some("wrong".into())), 1000, |_, _| panic!())
            .is_err());
        lights
            .configure(
                request(Some("test-only-light-authority".into())),
                1000,
                |_, camera| {
                    assert_eq!(camera, 0);
                    Ok(receipt(1000))
                },
            )
            .unwrap();
        for at in [2000, 3000, 4000] {
            lights
                .observe(sample(at, 0.0), at, |_, _| panic!("operator disabled auto"))
                .unwrap();
        }
        assert_eq!(lights.status().mode, LightMode::Off);
    }

    #[test]
    fn off_disarms_auto_even_when_the_off_write_fails() {
        let mut lights = Lighting::new(true, true, Some("test-only".into()));
        let result = lights.configure(
            LightRequest {
                mode: LightMode::Off,
                camera_pwm: None,
                chassis_pwm: None,
                token: Some("test-only".into()),
            },
            1000,
            |_, _| Err(anyhow!("disconnected")),
        );
        assert!(result.is_err());
        assert_eq!(lights.status().mode, LightMode::Off);
        for at in [2000, 3000, 4000] {
            lights
                .observe(sample(at, 0.16), at, |_, _| panic!("off must disarm auto"))
                .unwrap();
        }
        assert!(lights.status().events[0].receipt.is_none());
    }

    #[test]
    fn decoding_is_bounded_to_one_measurement_every_200_ms() {
        let mut lights = Lighting::new(true, true, None);
        assert!(lights.begin_measurement(1000));
        assert!(!lights.begin_measurement(2000)); // a decoder is still running
        lights.end_measurement();
        assert!(!lights.begin_measurement(1100));
        assert!(lights.begin_measurement(1200));
        assert!(!Lighting::new(false, false, None).begin_measurement(1000));
    }

    #[test]
    fn jpeg_measurement_uses_real_pixels_and_rejects_malformed_input() {
        let pixels = vec![41u8; 16 * 16]; // representative of the measured dark scene (~0.16)
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
            .encode(&pixels, 16, 16, image::ExtendedColorType::L8)
            .unwrap();
        let measured = measure_jpeg(&jpeg, 1000, 1010).unwrap();
        assert!((measured.mean_luma - 41.0 / 255.0).abs() < 0.01);
        assert!(measured.mean_luma < DARK);
        assert_eq!((measured.width, measured.height), (16, 16));
        assert_eq!(measured.jpeg_sha256.len(), 64);
        assert!(!measured.capture_timestamp_known);
        assert!(measure_jpeg(b"not a jpeg", 1000, 1010).is_err());
    }

    #[test]
    fn input_rejects_out_of_range_pwm_and_motion_fields() {
        for value in [
            serde_json::json!({"mode":"manual","camera_pwm":256}),
            serde_json::json!({"mode":"manual","camera_pwm":-1}),
            serde_json::json!({"mode":"manual","camera_pwm":180,"left":0.1}),
        ] {
            assert!(serde_json::from_value::<LightRequest>(value).is_err());
        }
    }
}
