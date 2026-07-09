use std::{
    collections::BTreeMap,
    process::{Child, Command, ExitStatus, Stdio},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::types::{ImageObservation, VisionResult};

pub const WORKER_FRAME_VERSION: &str = "leash-worker-frame-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ExternalWorkerSpec {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub restart_policy: WorkerRestartPolicy,
    pub max_restarts: u32,
    pub health_check: WorkerHealthCheck,
    pub required: bool,
}

impl ExternalWorkerSpec {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            restart_policy: WorkerRestartPolicy::Never,
            max_restarts: 0,
            health_check: WorkerHealthCheck::Process,
            required: true,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("external worker name cannot be empty");
        }
        if self.command.trim().is_empty() {
            bail!("external worker '{}' command cannot be empty", self.name);
        }
        for key in self.env.keys() {
            if key.trim().is_empty() {
                bail!("external worker '{}' env key cannot be empty", self.name);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum WorkerRestartPolicy {
    Never,
    OnFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum WorkerHealthCheck {
    Process,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ExternalWorkerState {
    Planned,
    Starting,
    Running,
    Exited,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct ExternalWorkerStatus {
    pub name: String,
    pub state: ExternalWorkerState,
    pub healthy: bool,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub restarts: u32,
    pub last_error: Option<String>,
    pub message: Option<String>,
    pub restart_policy: WorkerRestartPolicy,
    pub health_check: WorkerHealthCheck,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct WorkerInputFrame {
    pub schema_version: String,
    pub worker: String,
    pub sequence: u64,
    pub ts_ms: u128,
    pub payload: WorkerInputPayload,
}

impl WorkerInputFrame {
    pub fn perception(
        worker: impl Into<String>,
        sequence: u64,
        observation: ImageObservation,
    ) -> Self {
        Self {
            schema_version: WORKER_FRAME_VERSION.to_string(),
            worker: worker.into(),
            sequence,
            ts_ms: observation.ts_ms,
            payload: WorkerInputPayload::Perception { observation },
        }
    }

    pub fn validate(&self) -> Result<()> {
        validate_frame_identity(&self.schema_version, &self.worker)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum WorkerInputPayload {
    Perception { observation: ImageObservation },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct WorkerOutputFrame {
    pub schema_version: String,
    pub worker: String,
    pub sequence: u64,
    pub ts_ms: u128,
    pub payload: WorkerOutputPayload,
}

impl WorkerOutputFrame {
    pub fn vision(input: &WorkerInputFrame, result: VisionResult) -> Self {
        Self {
            schema_version: WORKER_FRAME_VERSION.to_string(),
            worker: input.worker.clone(),
            sequence: input.sequence,
            ts_ms: input.ts_ms,
            payload: WorkerOutputPayload::Vision { result },
        }
    }

    pub fn validate(&self) -> Result<()> {
        validate_frame_identity(&self.schema_version, &self.worker)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum WorkerOutputPayload {
    Vision {
        result: VisionResult,
    },
    Error {
        code: String,
        message: String,
        retryable: bool,
    },
}

pub fn simulated_perception_worker_status() -> ExternalWorkerStatus {
    ExternalWorkerStatus {
        name: "simulated-perception".to_string(),
        state: ExternalWorkerState::Running,
        healthy: true,
        pid: None,
        exit_code: None,
        restarts: 0,
        last_error: None,
        message: Some("deterministic in-process fixture".to_string()),
        restart_policy: WorkerRestartPolicy::Never,
        health_check: WorkerHealthCheck::Process,
        required: false,
    }
}

struct WorkerRuntime {
    spec: ExternalWorkerSpec,
    child: Option<Child>,
    status: ExternalWorkerStatus,
}

pub struct WorkerSupervisor {
    workers: BTreeMap<String, WorkerRuntime>,
}

impl WorkerSupervisor {
    pub fn new() -> Self {
        Self {
            workers: BTreeMap::new(),
        }
    }

    pub fn add(&mut self, spec: ExternalWorkerSpec) -> Result<()> {
        spec.validate()?;
        if self.workers.contains_key(&spec.name) {
            bail!("duplicate external worker '{}'", spec.name);
        }
        let status = ExternalWorkerStatus {
            name: spec.name.clone(),
            state: ExternalWorkerState::Planned,
            healthy: false,
            pid: None,
            exit_code: None,
            restarts: 0,
            last_error: None,
            message: Some("planned".to_string()),
            restart_policy: spec.restart_policy,
            health_check: spec.health_check,
            required: spec.required,
        };
        self.workers.insert(
            spec.name.clone(),
            WorkerRuntime {
                spec,
                child: None,
                status,
            },
        );
        Ok(())
    }

    pub fn start_all(&mut self) -> Result<()> {
        for name in self.workers.keys().cloned().collect::<Vec<_>>() {
            self.start(&name)?;
        }
        Ok(())
    }

    pub fn start(&mut self, name: &str) -> Result<()> {
        let runtime = self
            .workers
            .get_mut(name)
            .with_context(|| format!("unknown external worker '{name}'"))?;
        if runtime.child.is_some() {
            bail!("external worker '{name}' is already running");
        }
        runtime.status.state = ExternalWorkerState::Starting;
        runtime.status.healthy = false;
        runtime.status.message = Some("starting".to_string());
        let child = match spawn_child(&runtime.spec) {
            Ok(child) => child,
            Err(err) => {
                runtime.status.state = ExternalWorkerState::Failed;
                runtime.status.last_error = Some(err.to_string());
                runtime.status.message = Some("failed to start".to_string());
                return Err(err);
            }
        };
        runtime.status.pid = Some(child.id());
        runtime.status.exit_code = None;
        runtime.status.state = ExternalWorkerState::Running;
        runtime.status.healthy = true;
        runtime.status.last_error = None;
        runtime.status.message = Some("running".to_string());
        runtime.child = Some(child);
        Ok(())
    }

    pub fn poll_all(&mut self) -> Result<()> {
        for name in self.workers.keys().cloned().collect::<Vec<_>>() {
            self.poll(&name)?;
        }
        Ok(())
    }

    pub fn poll(&mut self, name: &str) -> Result<()> {
        let Some(runtime) = self.workers.get_mut(name) else {
            bail!("unknown external worker '{name}'");
        };
        let Some(child) = runtime.child.as_mut() else {
            return Ok(());
        };
        let Some(exit) = child.try_wait()? else {
            runtime.status.state = ExternalWorkerState::Running;
            runtime.status.healthy = true;
            runtime.status.message = Some("running".to_string());
            return Ok(());
        };

        let pid = runtime.status.pid;
        runtime.child = None;
        runtime.status.pid = pid;
        runtime.status.exit_code = exit.code();
        let exit_message = format!("exited {}", describe_exit(exit));
        runtime.status.last_error = (!exit.success()).then_some(exit_message.clone());

        if should_restart(&runtime.spec, &runtime.status, exit) {
            runtime.status.restarts += 1;
            let child = spawn_child(&runtime.spec)?;
            runtime.status.pid = Some(child.id());
            runtime.status.exit_code = None;
            runtime.status.state = ExternalWorkerState::Running;
            runtime.status.healthy = true;
            runtime.status.message = Some(format!(
                "restarted after exit {}; restart {}/{}",
                describe_exit(exit),
                runtime.status.restarts,
                runtime.spec.max_restarts
            ));
            runtime.child = Some(child);
        } else {
            runtime.status.state = ExternalWorkerState::Exited;
            runtime.status.healthy = false;
            runtime.status.message = Some(exit_message);
        }
        Ok(())
    }

    pub fn stop_all(&mut self) -> Result<()> {
        for name in self
            .workers
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            self.stop(&name)?;
        }
        Ok(())
    }

    pub fn stop(&mut self, name: &str) -> Result<()> {
        let Some(runtime) = self.workers.get_mut(name) else {
            bail!("unknown external worker '{name}'");
        };
        if let Some(mut child) = runtime.child.take() {
            match child.try_wait()? {
                Some(exit) => {
                    runtime.status.exit_code = exit.code();
                    runtime.status.message = Some(format!("exited {}", describe_exit(exit)));
                }
                None => {
                    child.kill()?;
                    let exit = child.wait()?;
                    runtime.status.exit_code = exit.code();
                    runtime.status.message = Some("stopped".to_string());
                }
            }
        } else {
            runtime.status.message = Some("stopped".to_string());
        }
        runtime.status.pid = None;
        runtime.status.state = ExternalWorkerState::Stopped;
        runtime.status.healthy = false;
        Ok(())
    }

    pub fn statuses(&self) -> Vec<ExternalWorkerStatus> {
        self.workers
            .values()
            .map(|runtime| runtime.status.clone())
            .collect()
    }

    pub fn status(&self, name: &str) -> Option<&ExternalWorkerStatus> {
        self.workers.get(name).map(|runtime| &runtime.status)
    }
}

impl Default for WorkerSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

fn spawn_child(spec: &ExternalWorkerSpec) -> Result<Child> {
    let mut command = Command::new(&spec.command);
    command
        .args(&spec.args)
        .envs(&spec.env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
        .spawn()
        .with_context(|| format!("failed to start external worker '{}'", spec.name))
}

fn should_restart(
    spec: &ExternalWorkerSpec,
    status: &ExternalWorkerStatus,
    exit: ExitStatus,
) -> bool {
    spec.restart_policy == WorkerRestartPolicy::OnFailure
        && !exit.success()
        && status.restarts < spec.max_restarts
}

fn describe_exit(exit: ExitStatus) -> String {
    match exit.code() {
        Some(code) => format!("code {code}"),
        None => "without exit code".to_string(),
    }
}

fn validate_frame_identity(schema_version: &str, worker: &str) -> Result<()> {
    if schema_version != WORKER_FRAME_VERSION {
        bail!("unsupported worker frame version '{schema_version}'");
    }
    if worker.trim().is_empty() {
        bail!("worker frame name cannot be empty");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread, time::Duration};

    fn shell_worker(name: &str, script: &str) -> ExternalWorkerSpec {
        let mut spec = ExternalWorkerSpec::new(name, "sh");
        spec.args = vec!["-c".to_string(), script.to_string()];
        spec
    }

    #[test]
    fn rejects_empty_worker_command() {
        let spec = ExternalWorkerSpec::new("empty", " ");
        let err = spec.validate().unwrap_err().to_string();
        assert!(err.contains("command cannot be empty"));
    }

    #[test]
    fn perception_worker_frames_round_trip_with_stable_identity() {
        let input = WorkerInputFrame::perception(
            "fixture",
            7,
            ImageObservation {
                ts_ms: 42,
                frame_id: "camera".to_string(),
                source: "sim".to_string(),
                width_px: 640,
                height_px: 480,
                content_type: "image/simulated".to_string(),
                byte_len: 0,
                sha256: None,
            },
        );
        input.validate().unwrap();
        let input_json = serde_json::to_string(&input).unwrap();
        let decoded: WorkerInputFrame = serde_json::from_str(&input_json).unwrap();
        assert_eq!(decoded, input);

        let output = WorkerOutputFrame::vision(
            &input,
            VisionResult {
                ok: true,
                status: "ok".to_string(),
                source: "fixture".to_string(),
                observed_at_ms: 42,
                duration_ms: 0,
                detections: Vec::new(),
                error: None,
            },
        );
        output.validate().unwrap();
        assert_eq!(output.sequence, input.sequence);
        assert_eq!(output.worker, input.worker);
    }

    #[test]
    fn bundled_simulated_perception_input_is_valid() {
        let input: WorkerInputFrame = serde_json::from_str(include_str!(
            "../examples/workers/sim-perception-input.json"
        ))
        .unwrap();

        input.validate().unwrap();
        assert_eq!(input.worker, "simulated-perception");
        assert_eq!(input.sequence, 42);
    }

    #[test]
    fn starts_reports_and_stops_worker_process() {
        let mut supervisor = WorkerSupervisor::new();
        supervisor
            .add(shell_worker("sleeper", "sleep 5"))
            .expect("valid worker");

        supervisor.start("sleeper").unwrap();
        supervisor.poll("sleeper").unwrap();

        let status = supervisor.status("sleeper").unwrap();
        assert_eq!(status.state, ExternalWorkerState::Running);
        assert!(status.healthy);
        assert!(status.pid.is_some());

        supervisor.stop("sleeper").unwrap();
        let status = supervisor.status("sleeper").unwrap();
        assert_eq!(status.state, ExternalWorkerState::Stopped);
        assert!(status.pid.is_none());
    }

    #[test]
    fn reports_exited_worker_process() {
        let mut supervisor = WorkerSupervisor::new();
        supervisor
            .add(shell_worker("exit-seven", "exit 7"))
            .expect("valid worker");

        supervisor.start("exit-seven").unwrap();
        thread::sleep(Duration::from_millis(50));
        supervisor.poll("exit-seven").unwrap();

        let status = supervisor.status("exit-seven").unwrap();
        assert_eq!(status.state, ExternalWorkerState::Exited);
        assert!(!status.healthy);
        assert_eq!(status.exit_code, Some(7));
        assert!(status
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("code 7"));
        assert!(status.message.as_deref().unwrap_or("").contains("code 7"));
    }

    #[test]
    fn reports_start_failure_without_exposing_worker_spec() {
        let mut supervisor = WorkerSupervisor::new();
        supervisor
            .add(ExternalWorkerSpec::new(
                "missing-worker",
                "leash-worker-command-that-does-not-exist",
            ))
            .unwrap();

        assert!(supervisor.start("missing-worker").is_err());
        let status = supervisor.status("missing-worker").unwrap();
        assert_eq!(status.state, ExternalWorkerState::Failed);
        assert!(!status.healthy);
        assert!(status.last_error.is_some());
        let json = serde_json::to_value(status).unwrap();
        assert!(json.get("command").is_none());
        assert!(json.get("args").is_none());
        assert!(json.get("env").is_none());
    }

    #[test]
    fn restarts_failed_worker_within_limit() {
        let mut spec = shell_worker("restart-once", "sleep 0.01; exit 9");
        spec.restart_policy = WorkerRestartPolicy::OnFailure;
        spec.max_restarts = 1;
        let mut supervisor = WorkerSupervisor::new();
        supervisor.add(spec).unwrap();

        supervisor.start("restart-once").unwrap();
        thread::sleep(Duration::from_millis(50));
        supervisor.poll("restart-once").unwrap();

        let status = supervisor.status("restart-once").unwrap();
        assert_eq!(status.state, ExternalWorkerState::Running);
        assert!(status.healthy);
        assert_eq!(status.restarts, 1);
        assert!(status
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("code 9"));

        supervisor.stop("restart-once").unwrap();
    }
}
