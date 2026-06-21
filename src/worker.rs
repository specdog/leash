use std::{
    collections::BTreeMap,
    process::{Child, Command, ExitStatus, Stdio},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

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
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub restarts: u32,
    pub message: Option<String>,
    pub restart_policy: WorkerRestartPolicy,
    pub health_check: WorkerHealthCheck,
    pub required: bool,
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
            pid: None,
            exit_code: None,
            restarts: 0,
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
        runtime.status.message = Some("starting".to_string());
        let child = spawn_child(&runtime.spec)?;
        runtime.status.pid = Some(child.id());
        runtime.status.exit_code = None;
        runtime.status.state = ExternalWorkerState::Running;
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
            runtime.status.message = Some("running".to_string());
            return Ok(());
        };

        let pid = runtime.status.pid;
        runtime.child = None;
        runtime.status.pid = pid;
        runtime.status.exit_code = exit.code();

        if should_restart(&runtime.spec, &runtime.status, exit) {
            runtime.status.restarts += 1;
            let child = spawn_child(&runtime.spec)?;
            runtime.status.pid = Some(child.id());
            runtime.status.exit_code = None;
            runtime.status.state = ExternalWorkerState::Running;
            runtime.status.message = Some(format!(
                "restarted after exit {}; restart {}/{}",
                describe_exit(exit),
                runtime.status.restarts,
                runtime.spec.max_restarts
            ));
            runtime.child = Some(child);
        } else {
            runtime.status.state = ExternalWorkerState::Exited;
            runtime.status.message = Some(format!("exited {}", describe_exit(exit)));
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
    fn starts_reports_and_stops_worker_process() {
        let mut supervisor = WorkerSupervisor::new();
        supervisor
            .add(shell_worker("sleeper", "sleep 5"))
            .expect("valid worker");

        supervisor.start("sleeper").unwrap();
        supervisor.poll("sleeper").unwrap();

        let status = supervisor.status("sleeper").unwrap();
        assert_eq!(status.state, ExternalWorkerState::Running);
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
        assert_eq!(status.exit_code, Some(7));
        assert!(status.message.as_deref().unwrap_or("").contains("code 7"));
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
        assert_eq!(status.restarts, 1);

        supervisor.stop("restart-once").unwrap();
    }
}
