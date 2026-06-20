use std::{
    collections::BTreeMap,
    env, fs,
    fs::File,
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_RUN_NAME: &str = "default";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub name: String,
    pub pid: u32,
    pub transport: String,
    pub profile: String,
    pub listen: String,
    pub log_path: PathBuf,
    pub args: Vec<String>,
    pub started_at_ms: u128,
    pub updated_at_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopOutcome {
    NotRunning,
    StoppedGracefully,
    Killed,
}

#[derive(Debug, Clone)]
pub struct RunRegistry {
    root: PathBuf,
}

impl RunRegistry {
    pub fn from_env() -> Result<Self> {
        Ok(Self::new(default_state_dir()?))
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn log_path(&self, name: &str) -> Result<PathBuf> {
        validate_run_name(name)?;
        Ok(self.root.join("logs").join(format!("{name}.log")))
    }

    pub fn write(&self, record: &RunRecord) -> Result<()> {
        validate_run_name(&record.name)?;
        fs::create_dir_all(self.run_dir())?;
        fs::create_dir_all(self.root.join("logs"))?;
        let path = self.record_path(&record.name)?;
        fs::write(path, serde_json::to_string_pretty(record)?)?;
        Ok(())
    }

    pub fn read(&self, name: &str) -> Result<Option<RunRecord>> {
        let path = self.record_path(name)?;
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("read run record {}", path.display()))?;
        Ok(Some(serde_json::from_str(&text)?))
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        let path = self.record_path(name)?;
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<RunRecord>> {
        let dir = self.run_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut records: Vec<RunRecord> = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let text = fs::read_to_string(entry.path())?;
            records.push(serde_json::from_str(&text)?);
        }
        records.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(records)
    }

    pub fn cleanup_stale(&self) -> Result<Vec<RunRecord>> {
        self.cleanup_stale_with(is_process_alive)
    }

    pub fn cleanup_stale_with(&self, alive: impl Fn(u32) -> bool) -> Result<Vec<RunRecord>> {
        let mut removed = Vec::new();
        for record in self.list()? {
            if !alive(record.pid) {
                self.remove(&record.name)?;
                removed.push(record);
            }
        }
        Ok(removed)
    }

    fn run_dir(&self) -> PathBuf {
        self.root.join("runs")
    }

    fn record_path(&self, name: &str) -> Result<PathBuf> {
        validate_run_name(name)?;
        Ok(self.run_dir().join(format!("{name}.json")))
    }
}

pub fn default_state_dir() -> Result<PathBuf> {
    let env = env::vars().collect();
    state_dir_from_env(&env)
}

pub fn state_dir_from_env(env: &BTreeMap<String, String>) -> Result<PathBuf> {
    if let Some(path) = env.get("LEASH_STATE_DIR").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = env.get("XDG_STATE_HOME").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path).join("leash"));
    }
    let Some(home) = env.get("HOME").filter(|path| !path.is_empty()) else {
        bail!("HOME, XDG_STATE_HOME, or LEASH_STATE_DIR is required for run state");
    };
    Ok(PathBuf::from(home).join(".local/state/leash"))
}

pub fn spawn_daemon(
    executable: &Path,
    args: &[String],
    log_path: &Path,
    run_id: &str,
) -> Result<u32> {
    let stdout = open_log_append(log_path)?;
    let stderr = stdout.try_clone()?;
    let child = Command::new(executable)
        .args(args)
        .env("LEASH_RUN_ID", run_id)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("spawn {}", executable.display()))?;
    Ok(child.id())
}

fn open_log_append(log_path: &Path) -> Result<File> {
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("open log {}", log_path.display()))
}

pub fn stop_process(pid: u32, graceful_timeout: Duration) -> Result<StopOutcome> {
    if !is_process_alive(pid) {
        return Ok(StopOutcome::NotRunning);
    }

    terminate_process(pid)?;
    let deadline = std::time::Instant::now() + graceful_timeout;
    while std::time::Instant::now() < deadline {
        if !is_process_alive(pid) {
            return Ok(StopOutcome::StoppedGracefully);
        }
        thread::sleep(Duration::from_millis(50));
    }

    kill_process(pid)?;
    Ok(StopOutcome::Killed)
}

pub fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub fn tail_file(path: &Path, lines: usize) -> Result<String> {
    if lines == 0 || !path.exists() {
        return Ok(String::new());
    }
    let mut file = fs::File::open(path)?;
    let len = file.seek(SeekFrom::End(0))?;
    let read_len = len.min(64 * 1024);
    file.seek(SeekFrom::End(-(read_len as i64)))?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let mut selected = text.lines().rev().take(lines).collect::<Vec<_>>();
    selected.reverse();
    Ok(selected.join("\n"))
}

pub fn tail_jsonl_file(path: &Path, lines: usize, module_filter: Option<&str>) -> Result<String> {
    if lines == 0 || !path.exists() {
        return Ok(String::new());
    }
    let text = fs::read_to_string(path)?;
    let mut selected = text
        .lines()
        .filter_map(|line| parse_log_line(line, module_filter))
        .collect::<Vec<_>>();
    if selected.len() > lines {
        selected = selected.split_off(selected.len() - lines);
    }
    Ok(selected.join("\n"))
}

fn parse_log_line(line: &str, module_filter: Option<&str>) -> Option<String> {
    let value = serde_json::from_str::<Value>(line).ok()?;
    if let Some(module_filter) = module_filter {
        let module = value.get("module")?.as_str()?;
        if !module_matches(module, module_filter) {
            return None;
        }
    }
    serde_json::to_string(&value).ok()
}

fn module_matches(module: &str, filter: &str) -> bool {
    module == filter || module.rsplit("::").next() == Some(filter)
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after unix epoch")
        .as_millis()
}

fn validate_run_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("run name cannot be empty");
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        bail!("run name may only contain letters, numbers, dash, underscore, and dot");
    }
    Ok(())
}

fn terminate_process(pid: u32) -> Result<()> {
    signal_process("-TERM", pid)
}

fn kill_process(pid: u32) -> Result<()> {
    signal_process("-KILL", pid)
}

fn signal_process(signal: &str, pid: u32) -> Result<()> {
    let status = Command::new("kill")
        .arg(signal)
        .arg(pid.to_string())
        .status()
        .with_context(|| format!("send {signal} to pid {pid}"))?;
    if !status.success() {
        bail!("failed to send {signal} to pid {pid}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_xdg_state_dir() {
        let env = BTreeMap::from([(
            "XDG_STATE_HOME".to_string(),
            "/tmp/leash-state-home".to_string(),
        )]);
        assert_eq!(
            state_dir_from_env(&env).unwrap(),
            PathBuf::from("/tmp/leash-state-home/leash")
        );
    }

    #[test]
    fn run_registry_round_trips_and_cleans_stale_records() {
        let dir = temp_dir("registry");
        let registry = RunRegistry::new(&dir);
        let record = RunRecord {
            name: "smoke".to_string(),
            pid: 42,
            transport: "http".to_string(),
            profile: "sim".to_string(),
            listen: "127.0.0.1:18080".to_string(),
            log_path: registry.log_path("smoke").unwrap(),
            args: vec!["serve".to_string(), "http".to_string()],
            started_at_ms: 1,
            updated_at_ms: 1,
        };

        registry.write(&record).unwrap();
        assert_eq!(registry.read("smoke").unwrap(), Some(record.clone()));

        let removed = registry.cleanup_stale_with(|_| false).unwrap();
        assert_eq!(removed, vec![record]);
        assert_eq!(registry.read("smoke").unwrap(), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn tail_file_returns_last_lines() {
        let dir = temp_dir("tail");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("run.log");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();
        assert_eq!(tail_file(&path, 2).unwrap(), "two\nthree");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn open_log_append_creates_parent_dir() {
        let dir = temp_dir("log-path");
        let path = dir.join("nested").join("run.log");

        drop(open_log_append(&path).unwrap());

        assert!(path.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn tail_jsonl_file_filters_by_module_and_line_count() {
        let dir = temp_dir("jsonl-tail");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("run.log");
        fs::write(
            &path,
            [
                r#"{"timestamp":1,"run_id":"smoke","module":"leash_harness::http","event":"ready","level":"info","fields":{}}"#,
                r#"{"timestamp":2,"run_id":"smoke","module":"leash_harness::runtime","event":"tick","level":"debug","fields":{}}"#,
                r#"{"timestamp":3,"run_id":"smoke","module":"leash_harness::runtime","event":"drive","level":"info","fields":{}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let text = tail_jsonl_file(&path, 1, Some("runtime")).unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 1);
        let value: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(value["event"], "drive");
        assert_eq!(value["module"], "leash_harness::runtime");

        let _ = fs::remove_dir_all(dir);
    }

    fn temp_dir(name: &str) -> PathBuf {
        env::temp_dir().join(format!("leash-{name}-{}-{}", std::process::id(), now_ms()))
    }
}
