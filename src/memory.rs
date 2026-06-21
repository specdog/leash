use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::{
    config::HarnessConfig,
    daemon,
    types::{SpatialMemoryEntry, SpatialMemoryKind, SpatialMemoryStatus},
};

pub const SPATIAL_MEMORY_FORMAT: &str = "leash-spatial-memory-v1";
pub const SPATIAL_MEMORY_STALE_AFTER_MS: u128 = 5 * 60 * 1000;

#[derive(Debug, Clone)]
pub struct SpatialMemoryTag {
    pub name: String,
    pub kind: SpatialMemoryKind,
    pub frame_id: String,
    pub x_m: f64,
    pub y_m: f64,
    pub confidence: f64,
}

#[derive(Debug, Clone)]
pub struct SpatialMemoryQuery {
    pub query: Option<String>,
    pub kind: Option<SpatialMemoryKind>,
    pub min_confidence: Option<f64>,
    pub include_stale: bool,
}

impl Default for SpatialMemoryQuery {
    fn default() -> Self {
        Self {
            query: None,
            kind: None,
            min_confidence: None,
            include_stale: true,
        }
    }
}

#[derive(Debug)]
pub struct SpatialMemoryStore {
    path: PathBuf,
    records: Mutex<Vec<SpatialMemoryRecord>>,
}

impl SpatialMemoryStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let records = if path.exists() {
            read_records(&path)?
        } else {
            Vec::new()
        };
        Ok(Self {
            path,
            records: Mutex::new(records),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn tag(&self, tag: SpatialMemoryTag) -> Result<SpatialMemoryStatus> {
        let tag = validate_tag(tag)?;
        let now = now_ms();
        let mut records = self.records.lock();
        if let Some(existing) = records
            .iter_mut()
            .find(|record| record.kind == tag.kind && record.name.eq_ignore_ascii_case(&tag.name))
        {
            existing.name = tag.name;
            existing.frame_id = tag.frame_id;
            existing.x_m = tag.x_m;
            existing.y_m = tag.y_m;
            existing.confidence = tag.confidence;
            existing.updated_at_ms = now;
        } else {
            records.push(SpatialMemoryRecord {
                name: tag.name,
                kind: tag.kind,
                frame_id: tag.frame_id,
                x_m: tag.x_m,
                y_m: tag.y_m,
                observed_at_ms: now,
                updated_at_ms: now,
                confidence: tag.confidence,
            });
        }
        records.sort_by(|left, right| {
            left.kind
                .as_str()
                .cmp(right.kind.as_str())
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        self.write_records(&records)?;
        Ok(self.status_from_records(&records, now))
    }

    pub fn list(&self) -> SpatialMemoryStatus {
        let records = self.records.lock();
        self.status_from_records(&records, now_ms())
    }

    pub fn query(&self, query: SpatialMemoryQuery) -> Result<SpatialMemoryStatus> {
        if let Some(min_confidence) = query.min_confidence {
            validate_confidence(min_confidence, "min_confidence")?;
        }
        let needle = query
            .query
            .as_deref()
            .map(str::trim)
            .filter(|query| !query.is_empty())
            .map(str::to_lowercase);
        let records = self.records.lock();
        let now = now_ms();
        let entries = records
            .iter()
            .map(|record| entry_from_record(record, now))
            .filter(|entry| query.kind.is_none_or(|kind| kind == entry.kind))
            .filter(|entry| query.include_stale || !entry.stale)
            .filter(|entry| {
                needle.as_ref().is_none_or(|needle| {
                    entry.name.to_lowercase().contains(needle)
                        || entry.frame_id.to_lowercase().contains(needle)
                })
            })
            .filter(|entry| {
                query
                    .min_confidence
                    .is_none_or(|min_confidence| entry.effective_confidence >= min_confidence)
            })
            .collect::<Vec<_>>();
        Ok(self.status_with_entries(entries))
    }

    pub fn clear(&self) -> Result<SpatialMemoryStatus> {
        let mut records = self.records.lock();
        records.clear();
        self.write_records(&records)?;
        Ok(self.status_from_records(&records, now_ms()))
    }

    #[cfg(test)]
    pub(crate) fn age_entry_for_test(
        &self,
        name: &str,
        kind: SpatialMemoryKind,
        age_ms: u128,
    ) -> Result<()> {
        let mut records = self.records.lock();
        let now = now_ms();
        let Some(record) = records
            .iter_mut()
            .find(|record| record.kind == kind && record.name.eq_ignore_ascii_case(name))
        else {
            bail!("memory entry not found");
        };
        record.updated_at_ms = now.saturating_sub(age_ms);
        self.write_records(&records)
    }

    fn status_from_records(
        &self,
        records: &[SpatialMemoryRecord],
        now: u128,
    ) -> SpatialMemoryStatus {
        self.status_with_entries(
            records
                .iter()
                .map(|record| entry_from_record(record, now))
                .collect(),
        )
    }

    fn status_with_entries(&self, entries: Vec<SpatialMemoryEntry>) -> SpatialMemoryStatus {
        SpatialMemoryStatus {
            ok: true,
            store_path: self.path.display().to_string(),
            count: entries.len(),
            entries,
        }
    }

    fn write_records(&self, records: &[SpatialMemoryRecord]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create memory directory {}", parent.display()))?;
        }
        let file = SpatialMemoryFile {
            format: SPATIAL_MEMORY_FORMAT.to_string(),
            entries: records.to_vec(),
        };
        let mut text = serde_json::to_string_pretty(&file)?;
        text.push('\n');
        fs::write(&self.path, text)
            .with_context(|| format!("write spatial memory {}", self.path.display()))
    }
}

pub fn default_spatial_memory_path(config: &HarnessConfig, instance_id: u64) -> PathBuf {
    let root = daemon::default_state_dir().unwrap_or_else(|_| env::temp_dir().join("leash"));
    let run_id = env::var("LEASH_RUN_ID")
        .ok()
        .map(|run_id| run_id.trim().to_string())
        .filter(|run_id| !run_id.is_empty())
        .unwrap_or_else(|| format!("pid{}-{instance_id}", std::process::id()));
    root.join("memory")
        .join(config.profile.as_str())
        .join(safe_path_segment(&config.role))
        .join(format!("{}.json", safe_path_segment(&run_id)))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpatialMemoryFile {
    format: String,
    entries: Vec<SpatialMemoryRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpatialMemoryRecord {
    name: String,
    kind: SpatialMemoryKind,
    frame_id: String,
    x_m: f64,
    y_m: f64,
    observed_at_ms: u128,
    updated_at_ms: u128,
    confidence: f64,
}

fn read_records(path: &Path) -> Result<Vec<SpatialMemoryRecord>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read spatial memory {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let file: SpatialMemoryFile = serde_json::from_str(&text)
        .with_context(|| format!("parse spatial memory {}", path.display()))?;
    if file.format != SPATIAL_MEMORY_FORMAT {
        bail!("unsupported spatial memory format '{}'", file.format);
    }
    Ok(file.entries.into_iter().map(normalize_record).collect())
}

fn validate_tag(tag: SpatialMemoryTag) -> Result<SpatialMemoryTag> {
    Ok(SpatialMemoryTag {
        name: validate_name(tag.name)?,
        kind: tag.kind,
        frame_id: validate_frame_id(tag.frame_id)?,
        x_m: validate_finite(tag.x_m, "x_m")?,
        y_m: validate_finite(tag.y_m, "y_m")?,
        confidence: validate_confidence(tag.confidence, "confidence")?,
    })
}

fn normalize_record(mut record: SpatialMemoryRecord) -> SpatialMemoryRecord {
    record.name = record.name.trim().to_string();
    record.frame_id = record.frame_id.trim().to_string();
    record.confidence = record.confidence.clamp(0.0, 1.0);
    record
}

fn validate_name(name: String) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("memory name cannot be empty");
    }
    Ok(name.to_string())
}

fn validate_frame_id(frame_id: String) -> Result<String> {
    let frame_id = frame_id.trim();
    if frame_id.is_empty() {
        bail!("frame_id cannot be empty");
    }
    Ok(frame_id.to_string())
}

fn validate_finite(value: f64, key: &str) -> Result<f64> {
    if !value.is_finite() {
        bail!("{key} must be finite");
    }
    Ok(value)
}

fn validate_confidence(value: f64, key: &str) -> Result<f64> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        bail!("{key} must be between 0.0 and 1.0");
    }
    Ok(value)
}

fn entry_from_record(record: &SpatialMemoryRecord, now: u128) -> SpatialMemoryEntry {
    let age_ms = now.saturating_sub(record.updated_at_ms);
    let stale = age_ms >= SPATIAL_MEMORY_STALE_AFTER_MS;
    SpatialMemoryEntry {
        name: record.name.clone(),
        kind: record.kind,
        frame_id: record.frame_id.clone(),
        x_m: record.x_m,
        y_m: record.y_m,
        observed_at_ms: record.observed_at_ms,
        updated_at_ms: record.updated_at_ms,
        confidence: round3(record.confidence),
        effective_confidence: if stale {
            round3(record.confidence * 0.5)
        } else {
            round3(record.confidence)
        },
        stale,
    }
}

fn safe_path_segment(value: &str) -> String {
    let value = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let value = value.trim_matches('.');
    if value.is_empty() {
        "default".to_string()
    } else {
        value.to_string()
    }
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after unix epoch")
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_update_query_stale_confidence_and_clear_are_file_backed() {
        let path = temp_memory_path("object-update");
        let store = SpatialMemoryStore::open(path.clone()).unwrap();

        let first = store
            .tag(SpatialMemoryTag {
                name: "charger".to_string(),
                kind: SpatialMemoryKind::Object,
                frame_id: "map".to_string(),
                x_m: 0.1,
                y_m: 0.2,
                confidence: 0.9,
            })
            .unwrap();
        assert_eq!(first.count, 1);
        assert!(path.exists());

        let second = store
            .tag(SpatialMemoryTag {
                name: "charger".to_string(),
                kind: SpatialMemoryKind::Object,
                frame_id: "map".to_string(),
                x_m: 0.4,
                y_m: 0.5,
                confidence: 0.8,
            })
            .unwrap();
        assert_eq!(second.count, 1);
        assert_eq!(second.entries[0].x_m, 0.4);
        assert_eq!(second.entries[0].confidence, 0.8);
        assert!(second.entries[0].updated_at_ms >= second.entries[0].observed_at_ms);

        let reloaded = SpatialMemoryStore::open(path.clone()).unwrap();
        assert_eq!(reloaded.list().entries[0].name, "charger");

        reloaded
            .age_entry_for_test(
                "charger",
                SpatialMemoryKind::Object,
                SPATIAL_MEMORY_STALE_AFTER_MS + 1,
            )
            .unwrap();
        let stale = reloaded
            .query(SpatialMemoryQuery {
                query: Some("charg".to_string()),
                kind: Some(SpatialMemoryKind::Object),
                min_confidence: Some(0.4),
                include_stale: true,
            })
            .unwrap();
        assert_eq!(stale.count, 1);
        assert!(stale.entries[0].stale);
        assert_eq!(stale.entries[0].effective_confidence, 0.4);

        let filtered = reloaded
            .query(SpatialMemoryQuery {
                query: Some("charg".to_string()),
                kind: Some(SpatialMemoryKind::Object),
                min_confidence: Some(0.6),
                include_stale: true,
            })
            .unwrap();
        assert_eq!(filtered.count, 0);

        let hidden = reloaded
            .query(SpatialMemoryQuery {
                query: Some("charg".to_string()),
                kind: Some(SpatialMemoryKind::Object),
                min_confidence: None,
                include_stale: false,
            })
            .unwrap();
        assert_eq!(hidden.count, 0);

        let cleared = reloaded.clear().unwrap();
        assert_eq!(cleared.count, 0);
        assert!(SpatialMemoryStore::open(path.clone())
            .unwrap()
            .list()
            .entries
            .is_empty());

        let _ = fs::remove_file(path);
    }

    fn temp_memory_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "leash-spatial-memory-{name}-{}-{}.json",
            std::process::id(),
            now_ms()
        ))
    }
}
