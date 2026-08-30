use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{
    config::HarnessConfig,
    memory::default_spatial_memory_path,
    types::{
        MapIdentity, PatrolZone, PatrolZoneList, PlannerGoal, Pose2d, SavedWaypoint,
        SavedWaypointList, SpeedMode, VisualizationPath, ZoneBoundaryPoint,
    },
};

pub const NAVIGATION_FORMAT: &str = "leash-navigation-v1";
pub const NAVIGATION_GOAL_SCHEMA_VERSION: &str = "leash.navigation-goal.v2";
pub const NAVIGATION_STATUS_SCHEMA_VERSION: &str = "leash.navigation-status.v2";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct NavigationGoalRequest {
    pub schema_version: String,
    pub mission_id: String,
    pub idempotency_key: String,
    pub token: String,
    pub approval: bool,
    pub expected_map: MapIdentity,
    pub frame_id: String,
    pub x_m: f64,
    pub y_m: f64,
    pub tolerance_m: f64,
    pub speed_mode: SpeedMode,
    pub deadline_ms: u128,
}

impl NavigationGoalRequest {
    pub fn planner_goal(&self) -> PlannerGoal {
        PlannerGoal {
            frame_id: self.frame_id.clone(),
            x_m: self.x_m,
            y_m: self.y_m,
            tolerance_m: self.tolerance_m,
            speed_mode: SpeedMode::Low,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum NavigationExecutorKind {
    SimGrid,
    Nav2,
    #[default]
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct NavigationReadiness {
    pub ready: bool,
    pub executor: NavigationExecutorKind,
    pub executor_supported: bool,
    pub map_matches: bool,
    pub localization_tracking: bool,
    pub lidar_available: bool,
    pub lidar_fresh: bool,
    pub collision_clear: bool,
    pub deadman_clear: bool,
    pub estop_clear: bool,
    pub message: String,
}

impl NavigationReadiness {
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self {
            ready: false,
            executor: NavigationExecutorKind::Unsupported,
            executor_supported: false,
            map_matches: false,
            localization_tracking: false,
            lidar_available: false,
            lidar_fresh: false,
            collision_clear: false,
            deadman_clear: false,
            estop_clear: false,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum NavigationTerminalReason {
    Reached,
    Cancelled,
    DeadlineExceeded,
    MapChanged,
    AuthorizationLost,
    SafetyStop,
    ExecutorUnavailable,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct NavigationFeedback {
    pub current_pose: Pose2d,
    pub distance_remaining_m: f64,
    pub received_at_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct NavigationExecutionStatus {
    pub ok: bool,
    pub active: bool,
    pub status: String,
    pub message: String,
    #[serde(default)]
    pub path: VisualizationPath,
    #[serde(default)]
    pub feedback: Option<NavigationFeedback>,
    #[serde(default)]
    pub terminal_reason: Option<NavigationTerminalReason>,
}

impl NavigationExecutionStatus {
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            active: false,
            status: "unsupported".to_string(),
            message: message.into(),
            path: VisualizationPath::default(),
            feedback: None,
            terminal_reason: Some(NavigationTerminalReason::ExecutorUnavailable),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct NavigationStatusResponse {
    pub schema_version: String,
    pub mission_id: String,
    pub idempotency_key: String,
    pub ok: bool,
    pub active: bool,
    pub status: String,
    pub message: String,
    pub expected_map: MapIdentity,
    #[serde(default)]
    pub active_map: Option<MapIdentity>,
    pub deadline_ms: u128,
    pub readiness: NavigationReadiness,
    #[serde(default)]
    pub terminal_reason: Option<NavigationTerminalReason>,
    #[serde(default)]
    pub goal: Option<PlannerGoal>,
    #[serde(default)]
    pub path: VisualizationPath,
    #[serde(default)]
    pub feedback: Option<NavigationFeedback>,
}

/// Physical navigation backend. Implementations may bridge to Nav2, but must
/// never write motors directly; velocity proposals still pass through Leash's
/// CPU safety supervisor and motor owner.
pub trait NavigationExecutor: Send + Sync {
    fn kind(&self) -> NavigationExecutorKind;

    fn readiness(&self) -> NavigationExecutionStatus;

    fn submit(
        &self,
        goal: &PlannerGoal,
        expected_map: &MapIdentity,
    ) -> Result<NavigationExecutionStatus>;

    fn cancel(&self) -> Result<NavigationExecutionStatus>;

    fn status(&self) -> NavigationExecutionStatus;
}

#[derive(Debug, Default)]
pub struct UnsupportedNavigationExecutor;

impl NavigationExecutor for UnsupportedNavigationExecutor {
    fn kind(&self) -> NavigationExecutorKind {
        NavigationExecutorKind::Unsupported
    }

    fn readiness(&self) -> NavigationExecutionStatus {
        NavigationExecutionStatus::unsupported(
            "physical navigation requires a configured Nav2 goal/cancel/feedback executor",
        )
    }

    fn submit(
        &self,
        _goal: &PlannerGoal,
        _expected_map: &MapIdentity,
    ) -> Result<NavigationExecutionStatus> {
        Ok(self.readiness())
    }

    fn cancel(&self) -> Result<NavigationExecutionStatus> {
        Ok(self.readiness())
    }

    fn status(&self) -> NavigationExecutionStatus {
        self.readiness()
    }
}

#[derive(Debug, Clone)]
pub struct WaypointSpec {
    pub id: String,
    pub name: String,
    pub frame_id: String,
    pub x_m: f64,
    pub y_m: f64,
    pub tolerance_m: f64,
}

#[derive(Debug, Clone)]
pub struct PatrolZoneSpec {
    pub id: String,
    pub name: String,
    pub frame_id: String,
    pub waypoint_ids: Vec<String>,
    pub boundary: Vec<ZoneBoundaryPoint>,
}

#[derive(Debug)]
pub struct NavigationStore {
    path: PathBuf,
    state: Mutex<NavigationFile>,
}

impl NavigationStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let state = if path.exists() {
            read_navigation(&path)?
        } else {
            NavigationFile::default()
        };
        validate_file(&state)?;
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn waypoints(&self) -> SavedWaypointList {
        self.waypoint_status(&self.state.lock())
    }

    pub fn waypoint(&self, id: &str) -> Option<SavedWaypoint> {
        self.state
            .lock()
            .waypoints
            .iter()
            .find(|waypoint| waypoint.id == id)
            .cloned()
    }

    pub fn create_waypoint(&self, spec: WaypointSpec) -> Result<SavedWaypointList> {
        self.create_waypoint_scoped(spec, None)
    }

    pub fn create_waypoint_scoped(
        &self,
        spec: WaypointSpec,
        map: Option<MapIdentity>,
    ) -> Result<SavedWaypointList> {
        let spec = validate_waypoint_spec(spec)?;
        let map = validate_map_scope(map, &spec.frame_id)?;
        let mut state = self.state.lock();
        if state
            .waypoints
            .iter()
            .any(|waypoint| waypoint.id == spec.id)
        {
            bail!("waypoint '{}' already exists", spec.id);
        }
        let now = now_ms();
        let mut next = state.clone();
        next.waypoints.push(SavedWaypoint {
            id: spec.id,
            name: spec.name,
            frame_id: spec.frame_id,
            map,
            x_m: spec.x_m,
            y_m: spec.y_m,
            tolerance_m: spec.tolerance_m,
            created_at_ms: now,
            updated_at_ms: now,
        });
        sort_file(&mut next);
        self.write(&next)?;
        *state = next;
        Ok(self.waypoint_status(&state))
    }

    pub fn update_waypoint(&self, spec: WaypointSpec) -> Result<SavedWaypointList> {
        self.update_waypoint_scoped(spec, None)
    }

    pub fn update_waypoint_scoped(
        &self,
        spec: WaypointSpec,
        map: Option<MapIdentity>,
    ) -> Result<SavedWaypointList> {
        let spec = validate_waypoint_spec(spec)?;
        let map = validate_map_scope(map, &spec.frame_id)?;
        let mut state = self.state.lock();
        let mut next = state.clone();
        let Some(waypoint) = next
            .waypoints
            .iter_mut()
            .find(|waypoint| waypoint.id == spec.id)
        else {
            bail!("waypoint '{}' does not exist", spec.id);
        };
        waypoint.name = spec.name;
        waypoint.frame_id = spec.frame_id;
        waypoint.map = map;
        waypoint.x_m = spec.x_m;
        waypoint.y_m = spec.y_m;
        waypoint.tolerance_m = spec.tolerance_m;
        waypoint.updated_at_ms = now_ms();
        validate_file(&next)?;
        sort_file(&mut next);
        self.write(&next)?;
        *state = next;
        Ok(self.waypoint_status(&state))
    }

    pub fn delete_waypoint(&self, id: &str) -> Result<SavedWaypointList> {
        let id = validate_id(id, "waypoint id")?;
        let mut state = self.state.lock();
        if let Some(zone) = state.zones.iter().find(|zone| {
            zone.waypoint_ids
                .iter()
                .any(|waypoint_id| waypoint_id == &id)
        }) {
            bail!("waypoint '{id}' is used by patrol zone '{}'", zone.id);
        }
        let mut next = state.clone();
        let original_len = next.waypoints.len();
        next.waypoints.retain(|waypoint| waypoint.id != id);
        if next.waypoints.len() == original_len {
            bail!("waypoint '{id}' does not exist");
        }
        self.write(&next)?;
        *state = next;
        Ok(self.waypoint_status(&state))
    }

    pub fn zones(&self) -> PatrolZoneList {
        self.zone_status(&self.state.lock())
    }

    pub fn zone(&self, id: &str) -> Option<PatrolZone> {
        self.state
            .lock()
            .zones
            .iter()
            .find(|zone| zone.id == id)
            .cloned()
    }

    pub fn create_zone(&self, spec: PatrolZoneSpec) -> Result<PatrolZoneList> {
        let spec = validate_zone_spec(spec)?;
        let mut state = self.state.lock();
        if state.zones.iter().any(|zone| zone.id == spec.id) {
            bail!("patrol zone '{}' already exists", spec.id);
        }
        ensure_waypoints_exist(&state, &spec.waypoint_ids)?;
        let now = now_ms();
        let mut next = state.clone();
        next.zones.push(PatrolZone {
            id: spec.id,
            name: spec.name,
            frame_id: spec.frame_id,
            waypoint_ids: spec.waypoint_ids,
            boundary: spec.boundary,
            created_at_ms: now,
            updated_at_ms: now,
        });
        sort_file(&mut next);
        self.write(&next)?;
        *state = next;
        Ok(self.zone_status(&state))
    }

    pub fn update_zone(&self, spec: PatrolZoneSpec) -> Result<PatrolZoneList> {
        let spec = validate_zone_spec(spec)?;
        let mut state = self.state.lock();
        ensure_waypoints_exist(&state, &spec.waypoint_ids)?;
        let mut next = state.clone();
        let Some(zone) = next.zones.iter_mut().find(|zone| zone.id == spec.id) else {
            bail!("patrol zone '{}' does not exist", spec.id);
        };
        zone.name = spec.name;
        zone.frame_id = spec.frame_id;
        zone.waypoint_ids = spec.waypoint_ids;
        zone.boundary = spec.boundary;
        zone.updated_at_ms = now_ms();
        sort_file(&mut next);
        self.write(&next)?;
        *state = next;
        Ok(self.zone_status(&state))
    }

    pub fn delete_zone(&self, id: &str) -> Result<PatrolZoneList> {
        let id = validate_id(id, "patrol zone id")?;
        let mut state = self.state.lock();
        let mut next = state.clone();
        let original_len = next.zones.len();
        next.zones.retain(|zone| zone.id != id);
        if next.zones.len() == original_len {
            bail!("patrol zone '{id}' does not exist");
        }
        self.write(&next)?;
        *state = next;
        Ok(self.zone_status(&state))
    }

    fn waypoint_status(&self, state: &NavigationFile) -> SavedWaypointList {
        SavedWaypointList {
            ok: true,
            store_path: self.path.display().to_string(),
            count: state.waypoints.len(),
            waypoints: state.waypoints.clone(),
        }
    }

    fn zone_status(&self, state: &NavigationFile) -> PatrolZoneList {
        PatrolZoneList {
            ok: true,
            store_path: self.path.display().to_string(),
            count: state.zones.len(),
            zones: state.zones.clone(),
        }
    }

    fn write(&self, state: &NavigationFile) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create navigation directory {}", parent.display()))?;
        }
        let mut text = serde_json::to_string_pretty(state)?;
        text.push('\n');
        fs::write(&self.path, text)
            .with_context(|| format!("write navigation store {}", self.path.display()))
    }
}

pub fn default_navigation_path(config: &HarnessConfig, instance_id: u64) -> PathBuf {
    navigation_path_for_memory(&default_spatial_memory_path(config, instance_id))
}

pub fn navigation_path_for_memory(memory_path: &Path) -> PathBuf {
    let stem = memory_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("leash");
    memory_path.with_file_name(format!("{stem}-navigation.json"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NavigationFile {
    format: String,
    waypoints: Vec<SavedWaypoint>,
    zones: Vec<PatrolZone>,
}

impl Default for NavigationFile {
    fn default() -> Self {
        Self {
            format: NAVIGATION_FORMAT.to_string(),
            waypoints: Vec::new(),
            zones: Vec::new(),
        }
    }
}

fn read_navigation(path: &Path) -> Result<NavigationFile> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read navigation store {}", path.display()))?;
    let state: NavigationFile = serde_json::from_str(&text)
        .with_context(|| format!("parse navigation store {}", path.display()))?;
    if state.format != NAVIGATION_FORMAT {
        bail!("unsupported navigation format '{}'", state.format);
    }
    Ok(state)
}

fn validate_file(state: &NavigationFile) -> Result<()> {
    let waypoint_ids = state
        .waypoints
        .iter()
        .map(|waypoint| waypoint.id.as_str())
        .collect::<HashSet<_>>();
    if waypoint_ids.len() != state.waypoints.len() {
        bail!("navigation store has duplicate waypoint ids");
    }
    let zone_ids = state
        .zones
        .iter()
        .map(|zone| zone.id.as_str())
        .collect::<HashSet<_>>();
    if zone_ids.len() != state.zones.len() {
        bail!("navigation store has duplicate patrol zone ids");
    }
    for waypoint in &state.waypoints {
        validate_waypoint_spec(WaypointSpec {
            id: waypoint.id.clone(),
            name: waypoint.name.clone(),
            frame_id: waypoint.frame_id.clone(),
            x_m: waypoint.x_m,
            y_m: waypoint.y_m,
            tolerance_m: waypoint.tolerance_m,
        })?;
        validate_map_scope(waypoint.map.clone(), &waypoint.frame_id)?;
    }
    for zone in &state.zones {
        validate_zone_spec(PatrolZoneSpec {
            id: zone.id.clone(),
            name: zone.name.clone(),
            frame_id: zone.frame_id.clone(),
            waypoint_ids: zone.waypoint_ids.clone(),
            boundary: zone.boundary.clone(),
        })?;
        for waypoint_id in &zone.waypoint_ids {
            if !waypoint_ids.contains(waypoint_id.as_str()) {
                bail!(
                    "patrol zone '{}' references missing waypoint '{waypoint_id}'",
                    zone.id
                );
            }
        }
    }
    Ok(())
}

fn validate_waypoint_spec(mut spec: WaypointSpec) -> Result<WaypointSpec> {
    spec.id = validate_id(&spec.id, "waypoint id")?;
    spec.name = validate_text(&spec.name, "waypoint name")?;
    spec.frame_id = validate_text(&spec.frame_id, "waypoint frame_id")?;
    validate_finite(spec.x_m, "waypoint x_m")?;
    validate_finite(spec.y_m, "waypoint y_m")?;
    if !spec.tolerance_m.is_finite() || spec.tolerance_m <= 0.0 {
        bail!("waypoint tolerance_m must be positive and finite");
    }
    Ok(spec)
}

fn validate_map_scope(map: Option<MapIdentity>, frame_id: &str) -> Result<Option<MapIdentity>> {
    if let Some(map) = &map {
        if map.map_id.trim().is_empty()
            || map.map_revision.trim().is_empty()
            || map.frame_id.trim().is_empty()
        {
            bail!("map scope requires map_id, map_revision, and frame_id");
        }
        if map.frame_id != frame_id {
            bail!("map scope frame_id does not match waypoint frame_id");
        }
    }
    Ok(map)
}

fn validate_zone_spec(mut spec: PatrolZoneSpec) -> Result<PatrolZoneSpec> {
    spec.id = validate_id(&spec.id, "patrol zone id")?;
    spec.name = validate_text(&spec.name, "patrol zone name")?;
    spec.frame_id = validate_text(&spec.frame_id, "patrol zone frame_id")?;
    if spec.waypoint_ids.is_empty() {
        bail!("patrol zone must include at least one waypoint id");
    }
    let mut seen = HashSet::new();
    for waypoint_id in &mut spec.waypoint_ids {
        *waypoint_id = validate_id(waypoint_id, "patrol zone waypoint id")?;
        if !seen.insert(waypoint_id.clone()) {
            bail!("patrol zone has duplicate waypoint id '{waypoint_id}'");
        }
    }
    if !spec.boundary.is_empty() && spec.boundary.len() < 3 {
        bail!("patrol zone boundary must be empty or contain at least three points");
    }
    for point in &spec.boundary {
        validate_finite(point.x_m, "patrol zone boundary x_m")?;
        validate_finite(point.y_m, "patrol zone boundary y_m")?;
    }
    Ok(spec)
}

fn ensure_waypoints_exist(state: &NavigationFile, waypoint_ids: &[String]) -> Result<()> {
    for waypoint_id in waypoint_ids {
        if !state
            .waypoints
            .iter()
            .any(|waypoint| waypoint.id == *waypoint_id)
        {
            bail!("patrol zone references missing waypoint '{waypoint_id}'");
        }
    }
    Ok(())
}

fn validate_id(value: &str, field: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 {
        bail!("{field} must contain 1 through 64 characters");
    }
    if !value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!("{field} may contain only letters, numbers, '-' and '_'");
    }
    Ok(value.to_string())
}

fn validate_text(value: &str, field: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("{field} cannot be empty");
    }
    Ok(value.to_string())
}

fn validate_finite(value: f64, field: &str) -> Result<()> {
    if !value.is_finite() {
        return Err(anyhow!("{field} must be finite"));
    }
    Ok(())
}

fn sort_file(state: &mut NavigationFile) {
    state
        .waypoints
        .sort_by(|left, right| left.id.cmp(&right.id));
    state.zones.sort_by(|left, right| left.id.cmp(&right.id));
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_v2_contract_pins_map_and_never_echoes_token() {
        let request: NavigationGoalRequest = serde_json::from_value(serde_json::json!({
            "schema_version": NAVIGATION_GOAL_SCHEMA_VERSION,
            "mission_id": "coverage-1",
            "idempotency_key": "coverage-1:goal-1",
            "token": "secret-pilot-token",
            "approval": true,
            "expected_map": {
                "map_id": "warehouse",
                "map_revision": "revision-a",
                "frame_id": "map"
            },
            "frame_id": "map",
            "x_m": 1.0,
            "y_m": 2.0,
            "tolerance_m": 0.2,
            "speed_mode": "low",
            "deadline_ms": 1234
        }))
        .unwrap();
        assert_eq!(request.expected_map.map_revision, "revision-a");

        let response = NavigationStatusResponse {
            schema_version: NAVIGATION_STATUS_SCHEMA_VERSION.to_string(),
            mission_id: request.mission_id,
            idempotency_key: request.idempotency_key,
            ok: false,
            active: false,
            status: "unsupported".to_string(),
            message: "Nav2 executor unavailable".to_string(),
            expected_map: request.expected_map,
            active_map: None,
            deadline_ms: request.deadline_ms,
            readiness: NavigationReadiness::unsupported("Nav2 executor unavailable"),
            terminal_reason: Some(NavigationTerminalReason::ExecutorUnavailable),
            goal: None,
            path: VisualizationPath::default(),
            feedback: None,
        };
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["schema_version"], NAVIGATION_STATUS_SCHEMA_VERSION);
        assert!(value.get("token").is_none());
        assert_eq!(value["terminal_reason"], "executor-unavailable");
    }

    #[test]
    fn waypoint_and_zone_crud_persists_and_preserves_references() {
        let path = std::env::temp_dir().join(format!(
            "leash-navigation-test-{}-{}.json",
            std::process::id(),
            now_ms()
        ));
        let store = NavigationStore::open(&path).unwrap();
        store
            .create_waypoint(WaypointSpec {
                id: "dock".to_string(),
                name: "Dock".to_string(),
                frame_id: "map".to_string(),
                x_m: 0.25,
                y_m: 0.25,
                tolerance_m: 0.1,
            })
            .unwrap();
        let updated = store
            .update_waypoint(WaypointSpec {
                id: "dock".to_string(),
                name: "Charging Dock".to_string(),
                frame_id: "map".to_string(),
                x_m: 0.25,
                y_m: 0.0,
                tolerance_m: 0.15,
            })
            .unwrap();
        assert_eq!(updated.waypoints[0].name, "Charging Dock");
        store
            .create_zone(PatrolZoneSpec {
                id: "entry".to_string(),
                name: "Entry".to_string(),
                frame_id: "map".to_string(),
                waypoint_ids: vec!["dock".to_string()],
                boundary: vec![
                    ZoneBoundaryPoint { x_m: 0.0, y_m: 0.0 },
                    ZoneBoundaryPoint { x_m: 1.0, y_m: 0.0 },
                    ZoneBoundaryPoint { x_m: 1.0, y_m: 1.0 },
                ],
            })
            .unwrap();
        let updated_zones = store
            .update_zone(PatrolZoneSpec {
                id: "entry".to_string(),
                name: "Entry Updated".to_string(),
                frame_id: "map".to_string(),
                waypoint_ids: vec!["dock".to_string()],
                boundary: Vec::new(),
            })
            .unwrap();
        assert_eq!(updated_zones.zones[0].name, "Entry Updated");

        assert!(store
            .delete_waypoint("dock")
            .unwrap_err()
            .to_string()
            .contains("used"));
        drop(store);

        let reopened = NavigationStore::open(&path).unwrap();
        assert_eq!(reopened.waypoints().count, 1);
        assert_eq!(reopened.zones().zones[0].waypoint_ids, vec!["dock"]);
        reopened.delete_zone("entry").unwrap();
        assert_eq!(reopened.delete_waypoint("dock").unwrap().count, 0);
        let _ = fs::remove_file(path);
    }
}
