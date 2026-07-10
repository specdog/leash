use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{mpsc, Arc},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::types::{
    CostmapFrame, LocalizationFrame, LocalizationStatus, MapMetadata, OccupancyGridFrame,
    TelemetryFrame,
};

pub const LOCALIZATION_PROVIDER_UPDATE_VERSION: &str = "leash-localization-provider-v1";
pub const DEFAULT_LOCALIZATION_STALE_AFTER_MS: u64 = 1_000;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum LocalizationProviderState {
    Initializing,
    Tracking,
    Degraded,
    Stale,
    Disconnected,
    Failed,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct LocalizationProviderStatus {
    pub provider: String,
    pub state: LocalizationProviderState,
    #[serde(default)]
    pub sequence: Option<u64>,
    pub generation: u64,
    #[serde(default)]
    pub last_update_ms: Option<u128>,
    #[serde(default)]
    pub last_received_ms: Option<u128>,
    pub stale_after_ms: u64,
    pub message: String,
    #[serde(default)]
    pub error: Option<String>,
}

impl Default for LocalizationProviderStatus {
    fn default() -> Self {
        Self {
            provider: "none".to_string(),
            state: LocalizationProviderState::Unavailable,
            sequence: None,
            generation: 0,
            last_update_ms: None,
            last_received_ms: None,
            stale_after_ms: DEFAULT_LOCALIZATION_STALE_AFTER_MS,
            message: "localization provider unavailable".to_string(),
            error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct LocalizationProviderUpdate {
    #[serde(default = "default_update_version")]
    pub version: String,
    pub sequence: u64,
    pub localization: LocalizationFrame,
    pub map: MapMetadata,
    pub occupancy_grid: OccupancyGridFrame,
    pub costmap: CostmapFrame,
}

impl LocalizationProviderUpdate {
    pub fn validate(&self) -> Result<(), LocalizationProviderError> {
        if self.version != LOCALIZATION_PROVIDER_UPDATE_VERSION {
            return Err(LocalizationProviderError::UnsupportedVersion);
        }
        self.localization
            .validate()
            .map_err(|error| LocalizationProviderError::InvalidLocalization(error.to_string()))?;
        if self.localization.map.map_id != self.map.map_id
            || self.localization.map.frame_id != self.map.frame_id
        {
            return Err(LocalizationProviderError::MapIdentityMismatch);
        }
        validate_grid(
            self.occupancy_grid.width,
            self.occupancy_grid.height,
            self.occupancy_grid.cells.len(),
            &self.occupancy_grid.metadata,
            &self.map,
        )?;
        validate_grid(
            self.costmap.width,
            self.costmap.height,
            self.costmap.costs.len(),
            &self.costmap.metadata,
            &self.map,
        )?;
        if self.occupancy_grid.frame_id != self.map.frame_id
            || self.costmap.frame_id != self.map.frame_id
        {
            return Err(LocalizationProviderError::GridFrameMismatch);
        }
        Ok(())
    }

    pub fn from_telemetry(sequence: u64, telemetry: &TelemetryFrame) -> Self {
        Self {
            version: default_update_version(),
            sequence,
            localization: telemetry.localization.clone(),
            map: telemetry.map.clone(),
            occupancy_grid: telemetry.occupancy_grid.clone(),
            costmap: telemetry.costmap.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct LocalizationProviderSnapshot {
    pub status: LocalizationProviderStatus,
    pub localization: LocalizationFrame,
    pub map: MapMetadata,
    pub occupancy_grid: OccupancyGridFrame,
    pub costmap: CostmapFrame,
}

pub trait LocalizationProvider: Send + Sync {
    fn name(&self) -> &str;
    fn snapshot(&self, now_ms: u128) -> LocalizationProviderSnapshot;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalizationApplyOutcome {
    Applied { generation: u64 },
    IgnoredOutOfOrder { current_sequence: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalizationProviderError {
    UnsupportedVersion,
    InvalidLocalization(String),
    MapIdentityMismatch,
    GridFrameMismatch,
    GridSizeMismatch,
    GridMetadataMismatch,
    ExternalQueueFull,
    ExternalQueueDisconnected,
}

impl std::fmt::Display for LocalizationProviderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion => {
                formatter.write_str("unsupported localization provider update version")
            }
            Self::InvalidLocalization(error) => {
                write!(formatter, "invalid localization update: {error}")
            }
            Self::MapIdentityMismatch => {
                formatter.write_str("localization and map identity do not match")
            }
            Self::GridFrameMismatch => {
                formatter.write_str("mapping grid frame does not match map frame")
            }
            Self::GridSizeMismatch => {
                formatter.write_str("mapping grid dimensions do not match cell count")
            }
            Self::GridMetadataMismatch => {
                formatter.write_str("mapping grid metadata does not match map metadata")
            }
            Self::ExternalQueueFull => {
                formatter.write_str("external localization update queue is full")
            }
            Self::ExternalQueueDisconnected => {
                formatter.write_str("external localization update queue is disconnected")
            }
        }
    }
}

impl std::error::Error for LocalizationProviderError {}

#[derive(Debug, Clone)]
pub struct InProcessLocalizationProvider {
    name: Arc<str>,
    stale_after_ms: u64,
    state: Arc<RwLock<ProviderState>>,
}

#[derive(Debug, Clone)]
struct ProviderState {
    status: LocalizationProviderStatus,
    update: Option<LocalizationProviderUpdate>,
}

impl InProcessLocalizationProvider {
    pub fn new(name: impl Into<String>, stale_after_ms: u64) -> Self {
        let name = name.into();
        let status = LocalizationProviderStatus {
            provider: name.clone(),
            stale_after_ms,
            message: "localization provider initialized".to_string(),
            state: LocalizationProviderState::Initializing,
            ..LocalizationProviderStatus::default()
        };
        Self {
            name: Arc::from(name),
            stale_after_ms,
            state: Arc::new(RwLock::new(ProviderState {
                status,
                update: None,
            })),
        }
    }

    pub fn apply(
        &self,
        update: LocalizationProviderUpdate,
    ) -> Result<LocalizationApplyOutcome, LocalizationProviderError> {
        self.apply_at(update, now_ms())
    }

    pub fn apply_at(
        &self,
        update: LocalizationProviderUpdate,
        received_at_ms: u128,
    ) -> Result<LocalizationApplyOutcome, LocalizationProviderError> {
        if let Err(error) = update.validate() {
            self.mark_failed(format!("malformed localization update: {error}"));
            return Err(error);
        }
        let mut state = self.state.write();
        if let Some(current_sequence) = state.status.sequence {
            if update.sequence <= current_sequence {
                return Ok(LocalizationApplyOutcome::IgnoredOutOfOrder { current_sequence });
            }
        }
        let map_changed = state
            .update
            .as_ref()
            .is_none_or(|current| current.localization.map != update.localization.map);
        if map_changed {
            state.status.generation = state.status.generation.saturating_add(1);
        }
        state.status.state = provider_state(update.localization.health.status);
        state.status.sequence = Some(update.sequence);
        state.status.last_update_ms = update.localization.health.last_update_ms;
        state.status.last_received_ms = Some(received_at_ms);
        state.status.message = update.localization.health.message.clone();
        state.status.error = update.localization.health.error.clone();
        state.update = Some(update);
        Ok(LocalizationApplyOutcome::Applied {
            generation: state.status.generation,
        })
    }

    pub fn mark_disconnected(&self, error: impl Into<String>) {
        self.mark_terminal(LocalizationProviderState::Disconnected, error);
    }

    pub fn mark_failed(&self, error: impl Into<String>) {
        self.mark_terminal(LocalizationProviderState::Failed, error);
    }

    fn mark_terminal(&self, provider_state: LocalizationProviderState, error: impl Into<String>) {
        let error = error.into();
        let mut state = self.state.write();
        state.status.state = provider_state;
        state.status.message = error.clone();
        state.status.error = Some(error.clone());
        if let Some(update) = &mut state.update {
            update.localization.health.status = LocalizationStatus::Lost;
            update.localization.health.message = error.clone();
            update.localization.health.error = Some(error);
        }
    }
}

impl LocalizationProvider for InProcessLocalizationProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn snapshot(&self, now_ms: u128) -> LocalizationProviderSnapshot {
        let state = self.state.read();
        let mut status = state.status.clone();
        let Some(update) = state.update.clone() else {
            return LocalizationProviderSnapshot {
                status,
                ..LocalizationProviderSnapshot::default()
            };
        };
        let mut localization = update.localization;
        if matches!(
            status.state,
            LocalizationProviderState::Tracking | LocalizationProviderState::Degraded
        ) && status
            .last_received_ms
            .is_some_and(|last| now_ms.saturating_sub(last) > self.stale_after_ms as u128)
        {
            status.state = LocalizationProviderState::Stale;
            status.message = "localization provider update is stale".to_string();
            status.error = None;
            localization.health.status = LocalizationStatus::Stale;
            localization.health.message = status.message.clone();
            localization.health.error = None;
        }
        LocalizationProviderSnapshot {
            status,
            localization,
            map: update.map,
            occupancy_grid: update.occupancy_grid,
            costmap: update.costmap,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SimulationLocalizationProvider(InProcessLocalizationProvider);

impl SimulationLocalizationProvider {
    pub fn new(stale_after_ms: u64) -> Self {
        Self(InProcessLocalizationProvider::new(
            "simulation",
            stale_after_ms,
        ))
    }

    pub fn publish(
        &self,
        update: LocalizationProviderUpdate,
    ) -> Result<LocalizationApplyOutcome, LocalizationProviderError> {
        self.0.apply(update)
    }
}

impl LocalizationProvider for SimulationLocalizationProvider {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn snapshot(&self, now_ms: u128) -> LocalizationProviderSnapshot {
        self.0.snapshot(now_ms)
    }
}

#[derive(Debug, Clone)]
pub struct ReplayLocalizationProvider(InProcessLocalizationProvider);

impl ReplayLocalizationProvider {
    pub fn new(stale_after_ms: u64) -> Self {
        Self(InProcessLocalizationProvider::new("replay", stale_after_ms))
    }

    pub fn publish_frame(
        &self,
        sequence: u64,
        frame: &TelemetryFrame,
    ) -> Result<LocalizationApplyOutcome, LocalizationProviderError> {
        self.0
            .apply(LocalizationProviderUpdate::from_telemetry(sequence, frame))
    }
}

impl LocalizationProvider for ReplayLocalizationProvider {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn snapshot(&self, now_ms: u128) -> LocalizationProviderSnapshot {
        self.0.snapshot(now_ms)
    }
}

#[derive(Debug, Clone)]
pub struct ExternalLocalizationProvider {
    provider: InProcessLocalizationProvider,
    sender: mpsc::SyncSender<ExternalMessage>,
}

#[derive(Debug)]
enum ExternalMessage {
    Update(Box<LocalizationProviderUpdate>, u128),
    Disconnect(String),
    Fail(String),
    #[cfg(test)]
    Panic,
}

impl ExternalLocalizationProvider {
    pub fn new(name: impl Into<String>, stale_after_ms: u64, capacity: usize) -> Self {
        Self::from_provider(
            InProcessLocalizationProvider::new(name, stale_after_ms),
            capacity,
        )
    }

    pub fn from_provider(provider: InProcessLocalizationProvider, capacity: usize) -> Self {
        let (sender, receiver) = mpsc::sync_channel(capacity.max(1));
        let worker_provider = provider.clone();
        thread::Builder::new()
            .name(format!("leash-localization-{}", provider.name()))
            .spawn(move || {
                let failure_provider = worker_provider.clone();
                if catch_unwind(AssertUnwindSafe(|| {
                    external_worker(worker_provider, receiver)
                }))
                .is_err()
                {
                    failure_provider.mark_failed("external localization provider worker panicked");
                }
            })
            .expect("spawn external localization provider worker");
        Self { provider, sender }
    }

    pub fn submit(
        &self,
        update: LocalizationProviderUpdate,
    ) -> Result<(), LocalizationProviderError> {
        self.try_send(ExternalMessage::Update(Box::new(update), now_ms()))
    }

    pub fn disconnect(&self, error: impl Into<String>) -> Result<(), LocalizationProviderError> {
        self.try_send(ExternalMessage::Disconnect(error.into()))
    }

    pub fn fail(&self, error: impl Into<String>) -> Result<(), LocalizationProviderError> {
        self.try_send(ExternalMessage::Fail(error.into()))
    }

    #[cfg(test)]
    fn panic_worker_for_test(&self) -> Result<(), LocalizationProviderError> {
        self.try_send(ExternalMessage::Panic)
    }

    fn try_send(&self, message: ExternalMessage) -> Result<(), LocalizationProviderError> {
        self.sender.try_send(message).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => LocalizationProviderError::ExternalQueueFull,
            mpsc::TrySendError::Disconnected(_) => {
                LocalizationProviderError::ExternalQueueDisconnected
            }
        })
    }
}

impl LocalizationProvider for ExternalLocalizationProvider {
    fn name(&self) -> &str {
        self.provider.name()
    }

    fn snapshot(&self, now_ms: u128) -> LocalizationProviderSnapshot {
        self.provider.snapshot(now_ms)
    }
}

fn external_worker(
    provider: InProcessLocalizationProvider,
    receiver: mpsc::Receiver<ExternalMessage>,
) {
    while let Ok(message) = receiver.recv() {
        match message {
            ExternalMessage::Update(update, received_at_ms) => {
                let _ = provider.apply_at(*update, received_at_ms);
            }
            ExternalMessage::Disconnect(error) => {
                provider.mark_disconnected(error);
            }
            ExternalMessage::Fail(error) => {
                provider.mark_failed(error);
            }
            #[cfg(test)]
            ExternalMessage::Panic => panic!("localization provider worker test panic"),
        }
    }
    provider.mark_disconnected("external localization stream disconnected");
}

fn validate_grid(
    width: u32,
    height: u32,
    cell_count: usize,
    metadata: &MapMetadata,
    map: &MapMetadata,
) -> Result<(), LocalizationProviderError> {
    if width as usize * height as usize != cell_count {
        return Err(LocalizationProviderError::GridSizeMismatch);
    }
    if metadata != map || width != map.width || height != map.height {
        return Err(LocalizationProviderError::GridMetadataMismatch);
    }
    Ok(())
}

fn provider_state(status: LocalizationStatus) -> LocalizationProviderState {
    match status {
        LocalizationStatus::Initializing => LocalizationProviderState::Initializing,
        LocalizationStatus::Tracking => LocalizationProviderState::Tracking,
        LocalizationStatus::Degraded => LocalizationProviderState::Degraded,
        LocalizationStatus::Stale => LocalizationProviderState::Stale,
        LocalizationStatus::Lost => LocalizationProviderState::Failed,
        LocalizationStatus::Unavailable => LocalizationProviderState::Unavailable,
    }
}

fn default_update_version() -> String {
    LOCALIZATION_PROVIDER_UPDATE_VERSION.to_string()
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
    use crate::{Harness, HarnessConfig};

    fn update(sequence: u64, revision: &str, ts_ms: u128) -> LocalizationProviderUpdate {
        let mut telemetry = Harness::new(HarnessConfig::default()).unwrap().telemetry();
        telemetry.localization.ts_ms = ts_ms;
        telemetry.localization.map.map_revision = revision.to_string();
        telemetry.localization.pose.as_mut().unwrap().pose.ts_ms = ts_ms;
        telemetry.localization.health.last_update_ms = Some(ts_ms);
        telemetry.map.ts_ms = ts_ms;
        telemetry.map.origin.ts_ms = ts_ms;
        telemetry.occupancy_grid.ts_ms = ts_ms;
        telemetry.occupancy_grid.origin.ts_ms = ts_ms;
        telemetry.occupancy_grid.metadata = telemetry.map.clone();
        telemetry.costmap.ts_ms = ts_ms;
        telemetry.costmap.origin.ts_ms = ts_ms;
        telemetry.costmap.metadata = telemetry.map.clone();
        LocalizationProviderUpdate::from_telemetry(sequence, &telemetry)
    }

    #[tokio::test]
    async fn in_process_provider_orders_updates_marks_stale_and_replaces_maps_atomically() {
        let provider = InProcessLocalizationProvider::new("test", 50);
        assert_eq!(
            provider.apply_at(update(2, "map-a", 100), 1_000).unwrap(),
            LocalizationApplyOutcome::Applied { generation: 1 }
        );
        assert_eq!(
            provider.apply_at(update(1, "old", 90), 1_001).unwrap(),
            LocalizationApplyOutcome::IgnoredOutOfOrder {
                current_sequence: 2
            }
        );
        let current = provider.snapshot(1_020);
        assert_eq!(current.localization.map.map_revision, "map-a");
        assert_eq!(current.status.state, LocalizationProviderState::Tracking);
        assert_eq!(
            provider.snapshot(1_051).status.state,
            LocalizationProviderState::Stale
        );

        assert_eq!(
            provider.apply_at(update(3, "map-b", 120), 1_060).unwrap(),
            LocalizationApplyOutcome::Applied { generation: 2 }
        );
        let replaced = provider.snapshot(1_061);
        assert_eq!(replaced.status.generation, 2);
        assert_eq!(replaced.localization.map.map_revision, "map-b");
        assert_eq!(replaced.occupancy_grid.metadata, replaced.map);
    }

    #[tokio::test]
    async fn malformed_and_failed_providers_degrade_without_panicking() {
        let provider = InProcessLocalizationProvider::new("test", 50);
        let mut malformed = update(1, "map-a", 100);
        malformed.occupancy_grid.cells.pop();
        assert_eq!(
            provider.apply_at(malformed, 1_000).unwrap_err(),
            LocalizationProviderError::GridSizeMismatch
        );
        assert_eq!(
            provider.snapshot(1_000).status.state,
            LocalizationProviderState::Failed
        );

        provider.apply_at(update(2, "map-a", 110), 1_010).unwrap();
        provider.mark_disconnected("transport closed");
        let disconnected = provider.snapshot(1_011);
        assert_eq!(
            disconnected.status.state,
            LocalizationProviderState::Disconnected
        );
        assert_eq!(
            disconnected.localization.health.status,
            LocalizationStatus::Lost
        );
    }

    #[tokio::test]
    async fn simulation_and_replay_use_the_same_provider_contract() {
        let simulation = SimulationLocalizationProvider::new(1_000);
        simulation.publish(update(1, "sim", 100)).unwrap();
        let replay = ReplayLocalizationProvider::new(1_000);
        let telemetry = Harness::new(HarnessConfig::default()).unwrap().telemetry();
        replay.publish_frame(1, &telemetry).unwrap();
        assert_eq!(simulation.snapshot(now_ms()).status.sequence, Some(1));
        assert_eq!(replay.snapshot(now_ms()).status.sequence, Some(1));
    }

    #[tokio::test]
    async fn external_provider_accepts_updates_without_blocking_and_reports_disconnect() {
        let provider = ExternalLocalizationProvider::new("external", 1_000, 4);
        provider.submit(update(1, "external", 100)).unwrap();
        for _ in 0..1_000 {
            if provider.snapshot(now_ms()).status.sequence == Some(1) {
                break;
            }
            thread::yield_now();
        }
        assert_eq!(provider.snapshot(now_ms()).status.sequence, Some(1));
        provider.disconnect("stream ended").unwrap();
        for _ in 0..1_000 {
            if provider.snapshot(now_ms()).status.state == LocalizationProviderState::Disconnected {
                break;
            }
            thread::yield_now();
        }
        assert_eq!(
            provider.snapshot(now_ms()).status.state,
            LocalizationProviderState::Disconnected
        );
        provider.submit(update(2, "external", 110)).unwrap();
        for _ in 0..1_000 {
            if provider.snapshot(now_ms()).status.sequence == Some(2) {
                break;
            }
            thread::yield_now();
        }
        let reconnected = provider.snapshot(now_ms());
        assert_eq!(reconnected.status.sequence, Some(2));
        assert_eq!(
            reconnected.status.state,
            LocalizationProviderState::Tracking
        );
    }

    #[tokio::test]
    async fn external_provider_worker_panic_is_isolated_as_failed_health() {
        let provider = ExternalLocalizationProvider::new("external-panic", 1_000, 1);
        provider.panic_worker_for_test().unwrap();
        for _ in 0..1_000 {
            if provider.snapshot(now_ms()).status.state == LocalizationProviderState::Failed {
                break;
            }
            thread::yield_now();
        }
        let snapshot = provider.snapshot(now_ms());
        assert_eq!(snapshot.status.state, LocalizationProviderState::Failed);
        assert!(snapshot
            .status
            .error
            .as_deref()
            .is_some_and(|error| error.contains("panicked")));
    }
}
