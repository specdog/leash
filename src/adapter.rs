use anyhow::{anyhow, Result};

use crate::types::{
    ImuSample, ImuStatus, PlanarRangeScan, Quaternion, RangeScanStatus, SensorDataStatus, Vector3Si,
};

pub trait MobileBaseAdapter: Send + Sync {
    fn drive(&self, left: f64, right: f64) -> Result<()>;

    fn stop(&self) -> Result<()> {
        self.drive(0.0, 0.0)
    }
}

pub trait GimbalAdapter: Send + Sync {
    fn aim_camera(&self, pan_deg: f64, tilt_deg: f64, speed: u32, accel: u32) -> Result<()> {
        let _ = (pan_deg, tilt_deg, speed, accel);
        Err(anyhow!("camera aim is unavailable for this adapter"))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CameraInputConfig {
    pub input_format: Option<String>,
    pub video_size: Option<String>,
    pub framerate: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraStreamCodec {
    Copy,
    Mjpeg { quality: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraCommandPlan {
    pub program: String,
    pub args: Vec<String>,
    pub content_type: String,
}

pub trait CameraAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn input_args(&self, device: &str, config: &CameraInputConfig) -> Vec<String>;
    fn capture_plan(&self, device: &str, config: &CameraInputConfig) -> CameraCommandPlan;
    fn stream_plan(
        &self,
        device: &str,
        config: &CameraInputConfig,
        codec: CameraStreamCodec,
    ) -> CameraCommandPlan;
}

/// Middleware-neutral source of timestamped planar range scans.
pub trait RangeScanAdapter: Send + Sync {
    fn range_scan(&self) -> RangeScanStatus;
}

/// Middleware-neutral source of timestamped SI-unit IMU samples.
pub trait ImuAdapter: Send + Sync {
    fn imu_sample(&self) -> ImuStatus;
}

pub fn simulated_range_scan(ts_ms: u128) -> RangeScanStatus {
    let sample = PlanarRangeScan {
        ts_ms,
        frame_id: "base_scan".to_string(),
        angle_min_rad: -std::f64::consts::FRAC_PI_2,
        angle_max_rad: std::f64::consts::FRAC_PI_2,
        angle_increment_rad: std::f64::consts::FRAC_PI_4,
        range_min_m: 0.05,
        range_max_m: 12.0,
        ranges_m: vec![Some(2.0), Some(1.5), Some(1.0), Some(1.5), Some(2.0)],
        intensities: vec![Some(10.0), Some(20.0), Some(30.0), Some(20.0), Some(10.0)],
    };
    RangeScanStatus {
        status: SensorDataStatus::Available,
        source: "sim".to_string(),
        last_ms: Some(ts_ms),
        sample: Some(sample),
        error: None,
    }
}

pub fn simulated_imu_sample(ts_ms: u128) -> ImuStatus {
    let sample = ImuSample {
        ts_ms,
        frame_id: "base_link".to_string(),
        linear_acceleration_mps2: Vector3Si {
            x: 0.0,
            y: 0.0,
            z: 9.80665,
        },
        angular_velocity_radps: Vector3Si::default(),
        orientation_xyzw: Some(Quaternion {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }),
    };
    ImuStatus {
        status: SensorDataStatus::Available,
        source: "sim".to_string(),
        last_ms: Some(ts_ms),
        sample: Some(sample),
        error: None,
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FfmpegV4l2CameraAdapter;

impl CameraAdapter for FfmpegV4l2CameraAdapter {
    fn name(&self) -> &'static str {
        "ffmpeg-v4l2"
    }

    fn input_args(&self, device: &str, config: &CameraInputConfig) -> Vec<String> {
        let mut args = vec!["-f".to_string(), "v4l2".to_string()];
        if let Some(format) = &config.input_format {
            args.extend(["-input_format".to_string(), format.clone()]);
        }
        if let Some(size) = &config.video_size {
            args.extend(["-video_size".to_string(), size.clone()]);
        }
        if let Some(framerate) = &config.framerate {
            args.extend(["-framerate".to_string(), framerate.clone()]);
        }
        args.extend(["-i".to_string(), device.to_string()]);
        args
    }

    fn capture_plan(&self, device: &str, config: &CameraInputConfig) -> CameraCommandPlan {
        let mut args = strings(&["-nostdin", "-hide_banner", "-loglevel", "error", "-y"]);
        args.extend(self.input_args(device, config));
        args.extend(strings(&[
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-vcodec",
            "mjpeg",
            "pipe:1",
        ]));
        CameraCommandPlan {
            program: "ffmpeg".to_string(),
            args,
            content_type: "image/jpeg".to_string(),
        }
    }

    fn stream_plan(
        &self,
        device: &str,
        config: &CameraInputConfig,
        codec: CameraStreamCodec,
    ) -> CameraCommandPlan {
        let mut args = strings(&["-nostdin", "-hide_banner", "-loglevel", "error"]);
        args.extend(self.input_args(device, config));
        args.push("-an".to_string());
        match codec {
            CameraStreamCodec::Copy => args.extend(strings(&["-c:v", "copy"])),
            CameraStreamCodec::Mjpeg { quality } => {
                args.extend(strings(&["-vcodec", "mjpeg", "-q:v"]));
                args.push(quality);
            }
        }
        args.extend(strings(&[
            "-f",
            "mpjpeg",
            "-boundary_tag",
            "leashframe",
            "pipe:1",
        ]));
        CameraCommandPlan {
            program: "ffmpeg".to_string(),
            args,
            content_type: "multipart/x-mixed-replace; boundary=leashframe".to_string(),
        }
    }
}

pub fn waveshare_drive_values(left: f64, right: f64, invert: bool, swap: bool) -> (f64, f64) {
    let (mut left, mut right) = if swap { (right, left) } else { (left, right) };
    if invert {
        left = -left;
        right = -right;
    }
    (left, right)
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct RecordingBase {
        commands: Mutex<Vec<(f64, f64)>>,
    }

    impl MobileBaseAdapter for RecordingBase {
        fn drive(&self, left: f64, right: f64) -> Result<()> {
            self.commands.lock().unwrap().push((left, right));
            Ok(())
        }
    }

    #[test]
    fn mobile_base_stop_uses_the_stable_zero_drive_contract() {
        let adapter = RecordingBase {
            commands: Mutex::new(Vec::new()),
        };
        adapter.drive(0.2, 0.1).unwrap();
        adapter.stop().unwrap();
        assert_eq!(
            adapter.commands.lock().unwrap().as_slice(),
            &[(0.2, 0.1), (0.0, 0.0)]
        );
    }

    #[test]
    fn waveshare_transform_preserves_swap_and_invert_behavior() {
        assert_eq!(waveshare_drive_values(0.2, -0.1, false, false), (0.2, -0.1));
        assert_eq!(waveshare_drive_values(0.2, -0.1, false, true), (-0.1, 0.2));
        assert_eq!(waveshare_drive_values(0.2, -0.1, true, false), (-0.2, 0.1));
        assert_eq!(waveshare_drive_values(0.2, -0.1, true, true), (0.1, -0.2));
    }

    #[test]
    fn default_gimbal_contract_fails_closed() {
        struct FixedBase;
        impl GimbalAdapter for FixedBase {}
        assert!(FixedBase.aim_camera(0.0, 0.0, 100, 10).is_err());
    }

    #[test]
    fn ffmpeg_camera_plans_keep_capture_and_stream_wire_shapes() {
        let adapter = FfmpegV4l2CameraAdapter;
        let config = CameraInputConfig {
            input_format: Some("mjpeg".to_string()),
            video_size: Some("640x480".to_string()),
            framerate: Some("30".to_string()),
        };
        let capture = adapter.capture_plan("/dev/video9", &config);
        assert_eq!(adapter.name(), "ffmpeg-v4l2");
        assert_eq!(capture.program, "ffmpeg");
        assert!(capture
            .args
            .windows(2)
            .any(|args| args == ["-i", "/dev/video9"]));
        assert!(capture
            .args
            .windows(2)
            .any(|args| args == ["-vcodec", "mjpeg"]));
        assert_eq!(capture.content_type, "image/jpeg");

        let stream = adapter.stream_plan(
            "/dev/video9",
            &config,
            CameraStreamCodec::Mjpeg {
                quality: "5".to_string(),
            },
        );
        assert!(stream.args.windows(2).any(|args| args == ["-q:v", "5"]));
        assert!(stream
            .args
            .windows(2)
            .any(|args| args == ["-boundary_tag", "leashframe"]));
        assert!(stream.content_type.contains("leashframe"));
    }

    #[test]
    fn simulated_sensor_fixtures_are_valid_and_deterministic() {
        let range_scan = simulated_range_scan(42);
        let imu = simulated_imu_sample(42);

        assert_eq!(range_scan, simulated_range_scan(42));
        assert_eq!(imu, simulated_imu_sample(42));
        range_scan.validate().unwrap();
        imu.validate().unwrap();
        assert_eq!(range_scan.sample.unwrap().ranges_m.len(), 5);
        assert_eq!(imu.sample.unwrap().frame_id, "base_link");
    }
}
