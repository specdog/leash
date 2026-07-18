use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process,
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    capability::{InvocationOrigin, SafetyClass},
    daemon::{default_state_dir, is_process_alive, now_ms, stop_process, tail_file, StopOutcome},
    types::AgentModelResponse,
    Harness,
};

pub const AGENT_SESSION_FORMAT: &str = "leash-agent-session-v1";
pub const AGENT_TASK_FORMAT: &str = "leash-agent-task-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AgentTurn {
    pub sequence: u64,
    pub started_at_ms: u128,
    pub prompt: String,
    pub response: AgentModelResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AgentSession {
    pub format: String,
    pub id: String,
    pub provider: String,
    pub model: String,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub turns: Vec<AgentTurn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AgentSessionSummary {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub turns: usize,
}

impl From<&AgentSession> for AgentSessionSummary {
    fn from(session: &AgentSession) -> Self {
        Self {
            id: session.id.clone(),
            provider: session.provider.clone(),
            model: session.model.clone(),
            created_at_ms: session.created_at_ms,
            updated_at_ms: session.updated_at_ms,
            turns: session.turns.len(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AgentRunOutput {
    pub ok: bool,
    pub session: AgentSessionSummary,
    pub turn: AgentTurn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AgentConsoleHealth {
    pub ok: bool,
    pub role: String,
    pub profile: String,
    pub mode: String,
    pub uptime_ms: u128,
    pub estop: bool,
    pub deadman_ok: bool,
    pub physical_actuation_enabled: bool,
    pub physical_navigation_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AgentConsoleCapability {
    pub name: String,
    pub module: String,
    pub safety: SafetyClass,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AgentTaskSnapshot {
    pub name: String,
    pub pid: u32,
    pub running: bool,
    pub state: AgentTaskState,
    pub capability: String,
    pub args: Value,
    pub interval_ms: u64,
    pub max_runs: u64,
    pub runs: u64,
    pub permissions: CapabilityPermissions,
    pub profile: String,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub last_error: Option<String>,
    pub recent_events: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AgentRuntimeSnapshot {
    pub ok: bool,
    pub ts_ms: u128,
    pub state_dir: String,
    pub health: AgentConsoleHealth,
    pub permissions: CapabilityPermissions,
    pub sessions: Vec<AgentSessionSummary>,
    pub selected_session: Option<AgentSession>,
    pub tasks: Vec<AgentTaskSnapshot>,
    pub capabilities: Vec<AgentConsoleCapability>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentTaskStopOutput {
    pub ok: bool,
    pub outcome: StopOutcome,
    pub task: AgentTaskRecord,
}

#[derive(Debug, Clone)]
pub struct AgentSessionStore {
    root: PathBuf,
}

impl AgentSessionStore {
    pub fn from_env() -> Result<Self> {
        Ok(Self::new(default_state_dir()?.join("agent")))
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn read(&self, id: &str) -> Result<Option<AgentSession>> {
        let path = self.session_path(id)?;
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("read agent session {}", path.display()))?;
        let session: AgentSession = serde_json::from_str(&text)?;
        if session.format != AGENT_SESSION_FORMAT {
            bail!("unsupported agent session format '{}'", session.format);
        }
        Ok(Some(session))
    }

    pub fn write(&self, session: &AgentSession) -> Result<()> {
        validate_name(&session.id, "agent session")?;
        fs::create_dir_all(self.session_dir())?;
        let path = self.session_path(&session.id)?;
        let temporary = path.with_extension(format!("json.tmp-{}", process::id()));
        fs::write(&temporary, serde_json::to_vec_pretty(session)?)?;
        fs::rename(&temporary, &path)?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<AgentSessionSummary>> {
        let directory = self.session_dir();
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut sessions = Vec::new();
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let text = fs::read_to_string(entry.path())?;
            let session: AgentSession = serde_json::from_str(&text)?;
            if session.format == AGENT_SESSION_FORMAT {
                sessions.push(AgentSessionSummary::from(&session));
            }
        }
        sessions.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(sessions)
    }

    pub fn latest(&self) -> Result<Option<AgentSession>> {
        let Some(summary) = self.list()?.into_iter().next() else {
            return Ok(None);
        };
        self.read(&summary.id)
    }

    fn session_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    fn session_path(&self, id: &str) -> Result<PathBuf> {
        validate_name(id, "agent session")?;
        Ok(self.session_dir().join(format!("{id}.json")))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct CapabilityPermissions {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

impl CapabilityPermissions {
    pub fn from_env() -> Result<Self> {
        Self::new(
            patterns_from_env("LEASH_AGENT_ALLOW_CAPABILITIES"),
            patterns_from_env("LEASH_AGENT_DENY_CAPABILITIES"),
        )
    }

    pub fn new(allow: Vec<String>, deny: Vec<String>) -> Result<Self> {
        let permissions = Self {
            allow: normalize_patterns(allow),
            deny: normalize_patterns(deny),
        };
        for pattern in permissions.allow.iter().chain(&permissions.deny) {
            validate_pattern(pattern)?;
        }
        Ok(permissions)
    }

    pub fn check(&self, capability: &str) -> Result<()> {
        if self
            .deny
            .iter()
            .any(|pattern| pattern_matches(pattern, capability))
        {
            bail!("agent capability '{capability}' is denied by LEASH agent permissions");
        }
        if !self.allow.is_empty()
            && !self
                .allow
                .iter()
                .any(|pattern| pattern_matches(pattern, capability))
        {
            bail!("agent capability '{capability}' is outside the configured allow list");
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct AgentRuntime {
    harness: Harness,
    sessions: AgentSessionStore,
    permissions: CapabilityPermissions,
}

impl AgentRuntime {
    pub fn from_env(harness: Harness, permissions: CapabilityPermissions) -> Result<Self> {
        Ok(Self::new(
            harness,
            AgentSessionStore::from_env()?,
            permissions,
        ))
    }

    pub fn new(
        harness: Harness,
        sessions: AgentSessionStore,
        permissions: CapabilityPermissions,
    ) -> Self {
        Self {
            harness,
            sessions,
            permissions,
        }
    }

    pub fn sessions(&self) -> &AgentSessionStore {
        &self.sessions
    }

    pub fn permissions(&self) -> &CapabilityPermissions {
        &self.permissions
    }

    pub fn run_prompt(
        &self,
        prompt: &str,
        session_id: Option<&str>,
        continue_last: bool,
    ) -> Result<AgentRunOutput> {
        if session_id.is_some() && continue_last {
            bail!("use either --session or --continue, not both");
        }
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("agent prompt cannot be empty");
        }

        let mut session = if continue_last {
            self.sessions
                .latest()?
                .ok_or_else(|| anyhow!("no prior agent session is available to continue"))?
        } else if let Some(id) = session_id {
            self.sessions
                .read(id)?
                .unwrap_or_else(|| self.new_session(id.to_string()))
        } else {
            self.new_session(generated_session_id())
        };

        let model_prompt = contextual_prompt(&session, prompt);
        let response = self
            .harness
            .agent_model_response(&model_prompt)?
            .ok_or_else(|| {
                anyhow!(
                    "agent provider '{}' does not support direct headless completion",
                    self.harness.config().agent_provider.as_str()
                )
            })?;
        let timestamp = now_ms();
        let turn = AgentTurn {
            sequence: session.turns.len() as u64 + 1,
            started_at_ms: timestamp,
            prompt: prompt.to_string(),
            response,
        };
        session.updated_at_ms = timestamp;
        session.turns.push(turn.clone());
        self.sessions.write(&session)?;

        Ok(AgentRunOutput {
            ok: true,
            session: AgentSessionSummary::from(&session),
            turn,
        })
    }

    pub fn invoke_capability(&self, capability: &str, args: Value) -> Result<Value> {
        self.permissions.check(capability)?;
        self.harness.capability_registry().invoke_value_with_origin(
            capability,
            args,
            InvocationOrigin::Agent,
        )
    }

    pub fn snapshot(
        &self,
        selected_session_id: Option<&str>,
        task_log_lines: usize,
    ) -> Result<AgentRuntimeSnapshot> {
        let health = self.harness.health();
        let selected_session = match selected_session_id {
            Some(id) => self.sessions.read(id)?,
            None => self.sessions.latest()?,
        };
        let task_store = AgentTaskStore::new(self.sessions.root());
        let tasks = task_store.snapshots(task_log_lines)?;
        let capabilities = self
            .harness
            .capability_registry()
            .descriptors()
            .iter()
            .map(|descriptor| AgentConsoleCapability {
                name: descriptor.name.clone(),
                module: descriptor.module.clone(),
                safety: descriptor.safety,
            })
            .collect();

        Ok(AgentRuntimeSnapshot {
            ok: true,
            ts_ms: now_ms(),
            state_dir: self.sessions.root().display().to_string(),
            health: AgentConsoleHealth {
                ok: health.ok,
                role: health.role,
                profile: health.profile,
                mode: health.mode,
                uptime_ms: health.uptime_ms,
                estop: health.estop,
                deadman_ok: health.deadman_ok,
                physical_actuation_enabled: health.physical_actuation_enabled,
                physical_navigation_enabled: health.physical_navigation_enabled,
            },
            permissions: self.permissions.clone(),
            sessions: self.sessions.list()?,
            selected_session,
            tasks,
            capabilities,
        })
    }

    pub async fn supervise_task(
        &self,
        store: &AgentTaskStore,
        name: &str,
    ) -> Result<AgentTaskRecord> {
        let mut record = wait_for_task_record(store, name).await?;
        record.state = AgentTaskState::Running;
        record.updated_at_ms = now_ms();
        store.write(&record)?;

        loop {
            let started_at_ms = now_ms();
            match self.invoke_capability(&record.capability, record.args.clone()) {
                Ok(result) => {
                    record.runs += 1;
                    record.last_result = Some(result.clone());
                    record.last_error = None;
                    record.updated_at_ms = now_ms();
                    println!(
                        "{}",
                        serde_json::to_string(&json!({
                            "type": "agent.task.run",
                            "task": record.name,
                            "run": record.runs,
                            "started_at_ms": started_at_ms,
                            "completed_at_ms": record.updated_at_ms,
                            "capability": record.capability,
                            "result": result,
                        }))?
                    );
                    io::stdout().flush()?;
                }
                Err(error) => {
                    record.state = AgentTaskState::Failed;
                    record.last_error = Some(error.to_string());
                    record.updated_at_ms = now_ms();
                    store.write(&record)?;
                    return Err(error);
                }
            }

            if record.max_runs != 0 && record.runs >= record.max_runs {
                record.state = AgentTaskState::Completed;
                store.write(&record)?;
                return Ok(record);
            }

            store.write(&record)?;
            tokio::time::sleep(Duration::from_millis(record.interval_ms)).await;
        }
    }

    fn new_session(&self, id: String) -> AgentSession {
        let timestamp = now_ms();
        AgentSession {
            format: AGENT_SESSION_FORMAT.to_string(),
            id,
            provider: self.harness.config().agent_provider.as_str().to_string(),
            model: self.harness.config().agent_model.clone(),
            created_at_ms: timestamp,
            updated_at_ms: timestamp,
            turns: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum AgentTaskState {
    Starting,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl AgentTaskState {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct AgentTaskRecord {
    pub format: String,
    pub name: String,
    pub pid: u32,
    pub capability: String,
    pub args: Value,
    pub interval_ms: u64,
    pub max_runs: u64,
    pub runs: u64,
    pub state: AgentTaskState,
    pub permissions: CapabilityPermissions,
    pub profile: String,
    pub log_path: PathBuf,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub last_result: Option<Value>,
    pub last_error: Option<String>,
}

impl AgentTaskRecord {
    pub fn running(&self) -> bool {
        self.state.is_active() && is_process_alive(self.pid)
    }
}

#[derive(Debug, Clone)]
pub struct AgentTaskStore {
    root: PathBuf,
}

impl AgentTaskStore {
    pub fn from_env() -> Result<Self> {
        Ok(Self::new(default_state_dir()?.join("agent")))
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn log_path(&self, name: &str) -> Result<PathBuf> {
        validate_name(name, "agent task")?;
        Ok(self.root.join("logs").join(format!("task-{name}.jsonl")))
    }

    pub fn read(&self, name: &str) -> Result<Option<AgentTaskRecord>> {
        let path = self.task_path(name)?;
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("read agent task {}", path.display()))?;
        let record: AgentTaskRecord = serde_json::from_str(&text)?;
        if record.format != AGENT_TASK_FORMAT {
            bail!("unsupported agent task format '{}'", record.format);
        }
        Ok(Some(record))
    }

    pub fn write(&self, record: &AgentTaskRecord) -> Result<()> {
        validate_name(&record.name, "agent task")?;
        fs::create_dir_all(self.task_dir())?;
        fs::create_dir_all(self.root.join("logs"))?;
        let path = self.task_path(&record.name)?;
        let temporary = path.with_extension(format!("json.tmp-{}", process::id()));
        fs::write(&temporary, serde_json::to_vec_pretty(record)?)?;
        fs::rename(&temporary, &path)?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<AgentTaskRecord>> {
        let directory = self.task_dir();
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.path().extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let text = fs::read_to_string(entry.path())?;
            let record: AgentTaskRecord = serde_json::from_str(&text)?;
            if record.format == AGENT_TASK_FORMAT {
                records.push(record);
            }
        }
        records.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(records)
    }

    pub fn refresh(&self, mut record: AgentTaskRecord) -> Result<AgentTaskRecord> {
        if record.state.is_active() && !is_process_alive(record.pid) {
            record.state = AgentTaskState::Failed;
            record.last_error = Some("supervisor process is no longer running".to_string());
            record.updated_at_ms = now_ms();
            self.write(&record)?;
        }
        Ok(record)
    }

    pub fn snapshots(&self, log_lines: usize) -> Result<Vec<AgentTaskSnapshot>> {
        self.list()?
            .into_iter()
            .map(|record| {
                let record = self.refresh(record)?;
                let recent_events = recent_task_events(&record.log_path, log_lines)?;
                let running = record.running();
                Ok(AgentTaskSnapshot {
                    name: record.name,
                    pid: record.pid,
                    running,
                    state: record.state,
                    capability: record.capability,
                    args: record.args,
                    interval_ms: record.interval_ms,
                    max_runs: record.max_runs,
                    runs: record.runs,
                    permissions: record.permissions,
                    profile: record.profile,
                    created_at_ms: record.created_at_ms,
                    updated_at_ms: record.updated_at_ms,
                    last_error: record.last_error,
                    recent_events,
                })
            })
            .collect()
    }

    pub fn stop(&self, name: &str, graceful_timeout: Duration) -> Result<AgentTaskStopOutput> {
        let mut record = self
            .read(name)?
            .ok_or_else(|| anyhow!("agent task '{name}' was not found"))?;
        let outcome = if record.state.is_active() {
            stop_process(record.pid, graceful_timeout)?
        } else {
            StopOutcome::NotRunning
        };
        if record.state.is_active() {
            record.state = AgentTaskState::Cancelled;
            record.updated_at_ms = now_ms();
            self.write(&record)?;
        }
        Ok(AgentTaskStopOutput {
            ok: true,
            outcome,
            task: record,
        })
    }

    fn task_dir(&self) -> PathBuf {
        self.root.join("tasks")
    }

    fn task_path(&self, name: &str) -> Result<PathBuf> {
        validate_name(name, "agent task")?;
        Ok(self.task_dir().join(format!("{name}.json")))
    }
}

fn recent_task_events(path: &Path, lines: usize) -> Result<Vec<Value>> {
    let text = tail_file(path, lines)?;
    Ok(text
        .lines()
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|_| json!({ "type": "log", "text": line }))
        })
        .collect())
}

fn contextual_prompt(session: &AgentSession, prompt: &str) -> String {
    if session.turns.is_empty() {
        return prompt.to_string();
    }
    let mut context = String::from("Continue this Leash agent session.\n\n");
    for turn in session.turns.iter().rev().take(20).rev() {
        context.push_str("Operator: ");
        context.push_str(&turn.prompt);
        context.push_str("\nAgent: ");
        context.push_str(&turn.response.text);
        context.push_str("\n\n");
    }
    context.push_str("Operator: ");
    context.push_str(prompt);
    context
}

async fn wait_for_task_record(store: &AgentTaskStore, name: &str) -> Result<AgentTaskRecord> {
    for _ in 0..40 {
        if let Some(record) = store.read(name)? {
            if record.pid == process::id() {
                return Ok(record);
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    bail!("agent task '{name}' did not receive its supervisor record")
}

fn generated_session_id() -> String {
    format!("session-{}-{}", now_ms(), process::id())
}

fn validate_name(value: &str, kind: &str) -> Result<()> {
    if value.is_empty() || value.len() > 80 {
        bail!("{kind} name must contain 1 to 80 characters");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("{kind} name may only contain letters, numbers, '-' and '_'");
    }
    Ok(())
}

fn patterns_from_env(key: &str) -> Vec<String> {
    std::env::var(key)
        .ok()
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn normalize_patterns(patterns: Vec<String>) -> Vec<String> {
    let mut patterns = patterns
        .into_iter()
        .map(|pattern| pattern.trim().to_string())
        .filter(|pattern| !pattern.is_empty())
        .collect::<Vec<_>>();
    patterns.sort();
    patterns.dedup();
    patterns
}

fn validate_pattern(pattern: &str) -> Result<()> {
    if pattern == "*" {
        return Ok(());
    }
    if pattern.is_empty() || pattern.strip_suffix('*').unwrap_or(pattern).contains('*') {
        bail!(
            "capability pattern '{pattern}' must be an exact name, '*', or a prefix ending in '*'"
        );
    }
    Ok(())
}

fn pattern_matches(pattern: &str, capability: &str) -> bool {
    pattern == "*"
        || pattern == capability
        || pattern
            .strip_suffix('*')
            .is_some_and(|prefix| capability.starts_with(prefix))
}

pub fn observe_only_capabilities(harness: &Harness) -> Vec<String> {
    harness
        .capability_registry()
        .descriptors()
        .iter()
        .filter(|descriptor| descriptor.safety == SafetyClass::ObserveOnly)
        .map(|descriptor| descriptor.name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("leash-agent-runtime-{label}-{}", now_ms()))
    }

    #[test]
    fn permission_deny_wins_and_prefix_allow_is_supported() {
        let permissions = CapabilityPermissions::new(
            vec!["planner_*".to_string()],
            vec!["planner_cancel".to_string()],
        )
        .unwrap();

        assert!(permissions.check("planner_status").is_ok());
        assert!(permissions.check("planner_cancel").is_err());
        assert!(permissions.check("health").is_err());
    }

    #[tokio::test]
    async fn session_resume_persists_context_and_turns() {
        let root = test_root("sessions");
        let runtime = AgentRuntime::new(
            Harness::new(crate::HarnessConfig::default()).unwrap(),
            AgentSessionStore::new(&root),
            CapabilityPermissions::default(),
        );

        let first = runtime
            .run_prompt("inspect the battery", Some("inspection"), false)
            .unwrap();
        let second = runtime
            .run_prompt("what did I ask?", Some("inspection"), false)
            .unwrap();

        assert_eq!(first.session.id, "inspection");
        assert_eq!(second.session.turns, 2);
        assert!(second
            .turn
            .response
            .prompt
            .contains("Operator: inspect the battery"));
        assert_eq!(
            runtime
                .sessions()
                .read("inspection")
                .unwrap()
                .unwrap()
                .turns
                .len(),
            2
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn capability_invocation_honors_agent_scope() {
        let root = test_root("scope");
        let runtime = AgentRuntime::new(
            Harness::new(crate::HarnessConfig::default()).unwrap(),
            AgentSessionStore::new(&root),
            CapabilityPermissions::new(vec!["health".to_string()], Vec::new()).unwrap(),
        );

        assert!(runtime.invoke_capability("health", json!({})).is_ok());
        assert!(runtime.invoke_capability("observe", json!({})).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn console_snapshot_joins_sessions_tasks_health_and_capabilities() {
        let root = test_root("console-snapshot");
        let permissions =
            CapabilityPermissions::new(vec!["planner_*".to_string()], Vec::new()).unwrap();
        let runtime = AgentRuntime::new(
            Harness::new(crate::HarnessConfig::default()).unwrap(),
            AgentSessionStore::new(&root),
            permissions.clone(),
        );
        runtime
            .run_prompt("report planner state", Some("operator-watch"), false)
            .unwrap();

        let store = AgentTaskStore::new(&root);
        let timestamp = now_ms();
        let log_path = store.log_path("planner-watch").unwrap();
        store
            .write(&AgentTaskRecord {
                format: AGENT_TASK_FORMAT.to_string(),
                name: "planner-watch".to_string(),
                pid: process::id(),
                capability: "planner_status".to_string(),
                args: json!({}),
                interval_ms: 2_000,
                max_runs: 1,
                runs: 1,
                state: AgentTaskState::Completed,
                permissions,
                profile: "sim".to_string(),
                log_path: log_path.clone(),
                created_at_ms: timestamp,
                updated_at_ms: timestamp,
                last_result: Some(json!({ "ok": true })),
                last_error: None,
            })
            .unwrap();
        fs::write(
            &log_path,
            serde_json::to_vec(&json!({ "type": "agent.task.run", "run": 1 })).unwrap(),
        )
        .unwrap();

        let snapshot = runtime.snapshot(Some("operator-watch"), 4).unwrap();

        assert!(snapshot.ok);
        assert_eq!(snapshot.selected_session.unwrap().id, "operator-watch");
        assert_eq!(snapshot.sessions.len(), 1);
        assert_eq!(snapshot.tasks.len(), 1);
        assert_eq!(snapshot.tasks[0].recent_events[0]["run"], 1);
        assert!(snapshot
            .capabilities
            .iter()
            .any(|capability| capability.name == "health"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stopping_a_completed_task_never_signals_its_stale_pid() {
        let root = test_root("completed-stop");
        let store = AgentTaskStore::new(&root);
        let timestamp = now_ms();
        store
            .write(&AgentTaskRecord {
                format: AGENT_TASK_FORMAT.to_string(),
                name: "finished".to_string(),
                pid: process::id(),
                capability: "health".to_string(),
                args: json!({}),
                interval_ms: 1_000,
                max_runs: 1,
                runs: 1,
                state: AgentTaskState::Completed,
                permissions: CapabilityPermissions::default(),
                profile: "sim".to_string(),
                log_path: store.log_path("finished").unwrap(),
                created_at_ms: timestamp,
                updated_at_ms: timestamp,
                last_result: Some(json!({ "ok": true })),
                last_error: None,
            })
            .unwrap();

        let stopped = store.stop("finished", Duration::from_millis(1)).unwrap();

        assert_eq!(stopped.outcome, StopOutcome::NotRunning);
        assert_eq!(stopped.task.state, AgentTaskState::Completed);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn finite_background_task_persists_each_run_and_completes() {
        let root = test_root("task");
        let permissions =
            CapabilityPermissions::new(vec!["planner_status".to_string()], Vec::new()).unwrap();
        let store = AgentTaskStore::new(&root);
        let timestamp = now_ms();
        store
            .write(&AgentTaskRecord {
                format: AGENT_TASK_FORMAT.to_string(),
                name: "planner-watch".to_string(),
                pid: process::id(),
                capability: "planner_status".to_string(),
                args: json!({}),
                interval_ms: 1,
                max_runs: 2,
                runs: 0,
                state: AgentTaskState::Starting,
                permissions: permissions.clone(),
                profile: "sim".to_string(),
                log_path: store.log_path("planner-watch").unwrap(),
                created_at_ms: timestamp,
                updated_at_ms: timestamp,
                last_result: None,
                last_error: None,
            })
            .unwrap();
        let runtime = AgentRuntime::new(
            Harness::new(crate::HarnessConfig::default()).unwrap(),
            AgentSessionStore::new(root.join("session-state")),
            permissions,
        );

        let completed = runtime
            .supervise_task(&store, "planner-watch")
            .await
            .unwrap();

        assert_eq!(completed.state, AgentTaskState::Completed);
        assert_eq!(completed.runs, 2);
        assert!(completed.last_result.is_some());
        assert_eq!(
            store.read("planner-watch").unwrap().unwrap().state,
            AgentTaskState::Completed
        );
        let _ = fs::remove_dir_all(root);
    }
}
