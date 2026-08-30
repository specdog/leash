//! Versioned map lifecycle and status contracts for external mission supervisors.
//!
//! Leash currently receives map state from the configured localization provider.
//! Starting, saving, and loading SLAM maps remains an operator-side operation, so
//! the HTTP lifecycle endpoint reports that boundary explicitly instead of
//! shelling out or pretending a request was applied.

use serde::{Deserialize, Serialize};

use crate::{
    localization::{LocalizationProviderSnapshot, LocalizationProviderState},
    types::MapIdentity,
};

pub const MAPPING_LIFECYCLE_SCHEMA_VERSION: &str = "leash.mapping-lifecycle.v1";
pub const MAPPING_STATUS_SCHEMA_VERSION: &str = "leash.mapping-status.v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum MappingLifecycleAction {
    StartNew,
    Stop,
    Save,
    Load,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MappingLifecycleRequest {
    pub schema_version: String,
    pub action: MappingLifecycleAction,
    #[serde(default)]
    pub map_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum MappingLifecycleState {
    Initializing,
    Tracking,
    Degraded,
    Stale,
    Disconnected,
    Failed,
    #[default]
    Unavailable,
}

impl From<LocalizationProviderState> for MappingLifecycleState {
    fn from(value: LocalizationProviderState) -> Self {
        match value {
            LocalizationProviderState::Initializing => Self::Initializing,
            LocalizationProviderState::Tracking => Self::Tracking,
            LocalizationProviderState::Degraded => Self::Degraded,
            LocalizationProviderState::Stale => Self::Stale,
            LocalizationProviderState::Disconnected => Self::Disconnected,
            LocalizationProviderState::Failed => Self::Failed,
            LocalizationProviderState::Unavailable => Self::Unavailable,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct MappingStatus {
    pub schema_version: String,
    pub ok: bool,
    pub lifecycle_control_supported: bool,
    pub state: MappingLifecycleState,
    #[serde(default)]
    pub active_map: Option<MapIdentity>,
    #[serde(default)]
    pub grid_revision: Option<String>,
    pub provider: String,
    #[serde(default)]
    pub provider_instance_id: Option<String>,
    #[serde(default)]
    pub last_update_ms: Option<u128>,
    pub message: String,
    #[serde(default)]
    pub error: Option<String>,
}

impl MappingStatus {
    pub fn from_snapshot(snapshot: &LocalizationProviderSnapshot) -> Self {
        let active_map = (!snapshot.localization.map.map_id.is_empty()
            && !snapshot.localization.map.map_revision.is_empty()
            && !snapshot.localization.map.frame_id.is_empty())
        .then(|| snapshot.localization.map.clone());
        let tracking = snapshot.status.state == LocalizationProviderState::Tracking;
        let map_ready = active_map.is_some()
            && snapshot.map.width > 0
            && snapshot.map.height > 0
            && snapshot.map.resolution_m.is_finite()
            && snapshot.map.resolution_m > 0.0
            && !snapshot.map.grid_revision.is_empty();
        let ready = tracking && map_ready;
        Self {
            schema_version: MAPPING_STATUS_SCHEMA_VERSION.to_string(),
            ok: ready,
            lifecycle_control_supported: false,
            state: snapshot.status.state.into(),
            active_map,
            grid_revision: (!snapshot.map.grid_revision.is_empty())
                .then(|| snapshot.map.grid_revision.clone()),
            provider: snapshot.status.provider.clone(),
            provider_instance_id: snapshot.status.provider_instance_id.clone(),
            last_update_ms: snapshot.status.last_update_ms,
            message: if ready {
                "fixed map and tracking localization are available".to_string()
            } else if tracking {
                "localization is tracking but fixed-map metadata is incomplete".to_string()
            } else {
                "mapping state is not ready for navigation".to_string()
            },
            error: snapshot.status.error.clone(),
        }
    }

    pub fn lifecycle_unsupported(mut self, action: MappingLifecycleAction) -> Self {
        self.ok = false;
        self.lifecycle_control_supported = false;
        self.message = format!(
            "mapping lifecycle action '{action:?}' is operator-managed and unsupported by this runtime"
        );
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        localization::{LocalizationProviderSnapshot, LocalizationProviderStatus},
        types::{LocalizationFrame, MapMetadata},
    };

    #[test]
    fn status_exposes_lineage_without_claiming_lifecycle_control() {
        let snapshot = LocalizationProviderSnapshot {
            status: LocalizationProviderStatus {
                provider: "slam-toolbox".to_string(),
                provider_instance_id: Some("boot-1".to_string()),
                state: LocalizationProviderState::Tracking,
                last_update_ms: Some(42),
                ..LocalizationProviderStatus::default()
            },
            localization: LocalizationFrame {
                map: MapIdentity {
                    map_id: "warehouse".to_string(),
                    map_revision: "revision-a".to_string(),
                    frame_id: "map".to_string(),
                },
                ..LocalizationFrame::default()
            },
            map: MapMetadata {
                grid_revision: "grid-9".to_string(),
                width: 4,
                height: 4,
                resolution_m: 0.05,
                ..MapMetadata::default()
            },
            ..LocalizationProviderSnapshot::default()
        };

        let status = MappingStatus::from_snapshot(&snapshot);
        assert!(status.ok);
        assert!(!status.lifecycle_control_supported);
        assert_eq!(status.active_map.unwrap().map_id, "warehouse");
        assert_eq!(status.grid_revision.as_deref(), Some("grid-9"));
    }
}
