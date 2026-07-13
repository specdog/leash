use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::types::VerifiedZeroEvidence;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum CalibrationPhase {
    Stationary,
    Straight,
    Turn,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibrationEnvelope {
    pub max_command: f64,
    pub max_wheel_travel_m: f64,
    pub deadline_ms: u64,
}

impl CalibrationPhase {
    pub fn envelope(self) -> CalibrationEnvelope {
        match self {
            Self::Stationary => CalibrationEnvelope {
                max_command: 0.0,
                max_wheel_travel_m: 0.02,
                deadline_ms: 90_000,
            },
            Self::Straight => CalibrationEnvelope {
                max_command: 0.18,
                max_wheel_travel_m: 1.25,
                deadline_ms: 30_000,
            },
            Self::Turn => CalibrationEnvelope {
                max_command: 0.16,
                max_wheel_travel_m: 0.75,
                deadline_ms: 45_000,
            },
            Self::Square => CalibrationEnvelope {
                max_command: 0.18,
                max_wheel_travel_m: 5.0,
                deadline_ms: 180_000,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CalibrationEnterRequest {
    pub token: String,
    pub approval: bool,
    pub calibration_sha256: String,
    pub phase: CalibrationPhase,
    pub run_index: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CalibrationStatus {
    pub active: bool,
    pub phase: Option<CalibrationPhase>,
    pub run_index: Option<u8>,
    pub calibration_sha256: Option<String>,
    pub entered_at_ms: Option<u128>,
    pub deadline_at_ms: Option<u128>,
    pub max_command: Option<f64>,
    pub max_wheel_travel_m: Option<f64>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CalibrationEnterResult {
    pub status: CalibrationStatus,
    pub verified_zero: VerifiedZeroEvidence,
}

impl Default for CalibrationStatus {
    fn default() -> Self {
        Self {
            active: false,
            phase: None,
            run_index: None,
            calibration_sha256: None,
            entered_at_ms: None,
            deadline_at_ms: None,
            max_command: None,
            max_wheel_travel_m: None,
            message: "calibration motion inactive".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CalibrationLease {
    request: CalibrationEnterRequest,
    entered_at_ms: u128,
    deadline_at_ms: u128,
    start_left_m: f64,
    start_right_m: f64,
}

impl CalibrationLease {
    pub(crate) fn enter(
        mut request: CalibrationEnterRequest,
        entered_at_ms: u128,
        start_left_m: f64,
        start_right_m: f64,
    ) -> Result<Self> {
        request.token = request.token.trim().to_string();
        if request.token.is_empty() {
            return Err(anyhow!("calibration token cannot be empty"));
        }
        if !request.approval {
            return Err(anyhow!("calibration entry requires explicit approval"));
        }
        if !is_lower_sha256(&request.calibration_sha256) {
            return Err(anyhow!(
                "calibration digest must be a lowercase SHA-256 value"
            ));
        }
        let valid_run_index = match request.phase {
            CalibrationPhase::Square => (1..=3).contains(&request.run_index),
            _ => request.run_index == 1,
        };
        if !valid_run_index {
            return Err(anyhow!("invalid calibration run index for phase"));
        }
        if !start_left_m.is_finite() || !start_right_m.is_finite() {
            return Err(anyhow!("calibration requires finite wheel odometry"));
        }
        let envelope = request.phase.envelope();
        Ok(Self {
            request,
            entered_at_ms,
            deadline_at_ms: entered_at_ms.saturating_add(u128::from(envelope.deadline_ms)),
            start_left_m,
            start_right_m,
        })
    }

    pub(crate) fn owner(&self) -> &str {
        &self.request.token
    }

    pub(crate) fn validate_drive(
        &mut self,
        token: &str,
        left: f64,
        right: f64,
        now_ms: u128,
        odometry_left_m: f64,
        odometry_right_m: f64,
    ) -> Result<()> {
        if token != self.request.token {
            return Err(anyhow!(
                "calibration command owner does not match active lease"
            ));
        }
        if !left.is_finite()
            || !right.is_finite()
            || !odometry_left_m.is_finite()
            || !odometry_right_m.is_finite()
        {
            return Err(anyhow!("calibration command and odometry must be finite"));
        }
        if now_ms > self.deadline_at_ms {
            return Err(anyhow!("calibration phase deadline exceeded"));
        }
        let envelope = self.request.phase.envelope();
        let travel = (odometry_left_m - self.start_left_m)
            .abs()
            .max((odometry_right_m - self.start_right_m).abs());
        if travel > envelope.max_wheel_travel_m {
            return Err(anyhow!("calibration phase wheel-travel bound exceeded"));
        }
        let stopped = left.abs() <= f64::EPSILON && right.abs() <= f64::EPSILON;
        if self.request.phase == CalibrationPhase::Stationary && !stopped {
            return Err(anyhow!("stationary calibration rejects non-zero commands"));
        }
        if left.abs() > envelope.max_command || right.abs() > envelope.max_command {
            return Err(anyhow!("calibration phase command bound exceeded"));
        }
        match self.request.phase {
            CalibrationPhase::Straight if !stopped && (left < 0.0 || right < 0.0) => Err(anyhow!(
                "straight calibration permits forward commands only"
            )),
            CalibrationPhase::Turn
                if !stopped
                    && (left.abs() <= f64::EPSILON
                        || right.abs() <= f64::EPSILON
                        || left.signum() == right.signum()) =>
            {
                Err(anyhow!(
                    "turn calibration requires opposite in-place wheel commands"
                ))
            }
            CalibrationPhase::Square
                if !(stopped
                    || left >= 0.0 && right >= 0.0
                    || left.signum() != right.signum()
                        && left.abs() > f64::EPSILON
                        && right.abs() > f64::EPSILON) =>
            {
                Err(anyhow!(
                    "square calibration permits forward or in-place turn commands only"
                ))
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn status(&self) -> CalibrationStatus {
        let envelope = self.request.phase.envelope();
        CalibrationStatus {
            active: true,
            phase: Some(self.request.phase),
            run_index: Some(self.request.run_index),
            calibration_sha256: Some(self.request.calibration_sha256.clone()),
            entered_at_ms: Some(self.entered_at_ms),
            deadline_at_ms: Some(self.deadline_at_ms),
            max_command: Some(envelope.max_command),
            max_wheel_travel_m: Some(envelope.max_wheel_travel_m),
            message: "bounded calibration motion active".to_string(),
        }
    }
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(phase: CalibrationPhase, run_index: u8) -> CalibrationEnterRequest {
        CalibrationEnterRequest {
            token: "calibration-owner".to_string(),
            approval: true,
            calibration_sha256: "a".repeat(64),
            phase,
            run_index,
        }
    }

    fn valid_command(phase: CalibrationPhase, max_command: f64) -> (f64, f64) {
        match phase {
            CalibrationPhase::Stationary => (0.0, 0.0),
            CalibrationPhase::Straight | CalibrationPhase::Square => {
                (max_command * 0.5, max_command * 0.5)
            }
            CalibrationPhase::Turn => (-max_command * 0.5, max_command * 0.5),
        }
    }

    #[test]
    fn stationary_phase_rejects_non_zero_commands() {
        let mut lease =
            CalibrationLease::enter(request(CalibrationPhase::Stationary, 1), 1_000, 0.0, 0.0)
                .unwrap();

        let error = lease
            .validate_drive("calibration-owner", 0.01, 0.01, 1_001, 0.0, 0.0)
            .unwrap_err()
            .to_string();

        assert!(error.contains("stationary"));
    }

    #[test]
    fn every_phase_enforces_command_travel_and_deadline_bounds() {
        for phase in [
            CalibrationPhase::Straight,
            CalibrationPhase::Turn,
            CalibrationPhase::Square,
        ] {
            let run_index = if phase == CalibrationPhase::Square {
                2
            } else {
                1
            };
            let envelope = phase.envelope();
            let (left, right) = valid_command(phase, envelope.max_command);
            let mut command_lease =
                CalibrationLease::enter(request(phase, run_index), 1_000, 0.0, 0.0).unwrap();
            assert!(command_lease
                .validate_drive("calibration-owner", left, right, 1_001, 0.0, 0.0,)
                .is_ok());
            assert!(command_lease
                .validate_drive(
                    "calibration-owner",
                    envelope.max_command + 0.001,
                    right,
                    1_002,
                    0.0,
                    0.0,
                )
                .is_err());

            let mut travel_lease =
                CalibrationLease::enter(request(phase, run_index), 2_000, 0.0, 0.0).unwrap();
            assert!(travel_lease
                .validate_drive(
                    "calibration-owner",
                    left,
                    right,
                    2_001,
                    envelope.max_wheel_travel_m + 0.001,
                    0.0,
                )
                .is_err());

            let mut deadline_lease =
                CalibrationLease::enter(request(phase, run_index), 3_000, 0.0, 0.0).unwrap();
            assert!(deadline_lease
                .validate_drive(
                    "calibration-owner",
                    left,
                    right,
                    3_000 + u128::from(envelope.deadline_ms) + 1,
                    0.0,
                    0.0,
                )
                .is_err());
        }
    }

    #[test]
    fn lease_requires_exact_owner_digest_approval_and_run_index() {
        assert!(
            CalibrationLease::enter(request(CalibrationPhase::Square, 0), 1_000, 0.0, 0.0,)
                .is_err()
        );
        assert!(
            CalibrationLease::enter(request(CalibrationPhase::Straight, 2), 1_000, 0.0, 0.0,)
                .is_err()
        );

        let mut invalid_digest = request(CalibrationPhase::Straight, 1);
        invalid_digest.calibration_sha256 = "A".repeat(64);
        assert!(CalibrationLease::enter(invalid_digest, 1_000, 0.0, 0.0).is_err());

        let mut no_approval = request(CalibrationPhase::Straight, 1);
        no_approval.approval = false;
        assert!(CalibrationLease::enter(no_approval, 1_000, 0.0, 0.0).is_err());

        let mut lease =
            CalibrationLease::enter(request(CalibrationPhase::Straight, 1), 1_000, 0.0, 0.0)
                .unwrap();
        assert!(lease
            .validate_drive("wrong-owner", 0.05, 0.05, 1_001, 0.0, 0.0)
            .is_err());
    }
}
