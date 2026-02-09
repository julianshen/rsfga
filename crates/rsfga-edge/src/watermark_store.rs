//! Persistent watermark storage for edge sync daemon.
//!
//! The `WatermarkStore` trait abstracts watermark persistence so the edge
//! daemon can survive restarts without replaying all events from NATS origin.
//!
//! Implementations:
//! - `FileWatermarkStore`: JSON file on disk with atomic writes (temp + rename)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::{EdgeError, Result};

/// Persistent watermark positions to save/load.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct WatermarkSnapshot {
    /// Map of store_id -> contiguous sequence position.
    pub positions: HashMap<String, u64>,
}

/// Trait for persisting sync watermark state across daemon restarts.
#[async_trait]
pub trait WatermarkStore: Send + Sync {
    /// Save the current watermark positions.
    async fn save(&self, snapshot: &WatermarkSnapshot) -> Result<()>;

    /// Load previously saved watermark positions.
    /// Returns an empty snapshot if no prior state exists.
    async fn load(&self) -> Result<WatermarkSnapshot>;
}

/// File-based watermark store using atomic JSON writes.
///
/// Writes are performed atomically via temp-file + rename to prevent
/// corruption on crash. The file is a simple JSON object mapping
/// store IDs to their contiguous sequence positions.
#[derive(Debug, Clone)]
pub struct FileWatermarkStore {
    path: PathBuf,
}

impl FileWatermarkStore {
    /// Create a new file-based watermark store.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Get the path to the watermark file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait]
impl WatermarkStore for FileWatermarkStore {
    async fn save(&self, snapshot: &WatermarkSnapshot) -> Result<()> {
        let json = serde_json::to_string_pretty(snapshot)?;
        let tmp_path = self.path.with_extension("tmp");

        // Write to temp file first
        tokio::fs::write(&tmp_path, json.as_bytes())
            .await
            .map_err(EdgeError::Io)?;

        // Atomic rename
        tokio::fs::rename(&tmp_path, &self.path)
            .await
            .map_err(EdgeError::Io)?;

        debug!(
            path = %self.path.display(),
            stores = snapshot.positions.len(),
            "Saved watermark snapshot"
        );

        Ok(())
    }

    async fn load(&self) -> Result<WatermarkSnapshot> {
        if !self.path.exists() {
            info!(
                path = %self.path.display(),
                "No watermark file found, starting from empty state"
            );
            return Ok(WatermarkSnapshot::default());
        }

        let bytes = tokio::fs::read(&self.path).await.map_err(EdgeError::Io)?;
        let snapshot: WatermarkSnapshot = serde_json::from_slice(&bytes).map_err(|e| {
            warn!(
                error = %e,
                path = %self.path.display(),
                "Failed to parse watermark file, starting from empty state"
            );
            e
        })?;

        info!(
            path = %self.path.display(),
            stores = snapshot.positions.len(),
            positions = ?snapshot.positions,
            "Loaded watermark snapshot"
        );

        Ok(snapshot)
    }
}

/// In-memory watermark store for testing.
#[derive(Debug, Default)]
pub struct InMemoryWatermarkStore {
    snapshot: tokio::sync::RwLock<WatermarkSnapshot>,
}

impl InMemoryWatermarkStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl WatermarkStore for InMemoryWatermarkStore {
    async fn save(&self, snapshot: &WatermarkSnapshot) -> Result<()> {
        *self.snapshot.write().await = snapshot.clone();
        Ok(())
    }

    async fn load(&self) -> Result<WatermarkSnapshot> {
        Ok(self.snapshot.read().await.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ===========================================
    // Section 1: WatermarkSnapshot Tests
    // ===========================================

    #[test]
    fn test_watermark_snapshot_default_is_empty() {
        let snapshot = WatermarkSnapshot::default();
        assert!(snapshot.positions.is_empty());
    }

    #[test]
    fn test_watermark_snapshot_serialization_roundtrip() {
        let mut snapshot = WatermarkSnapshot::default();
        snapshot.positions.insert("store-1".to_string(), 42);
        snapshot.positions.insert("store-2".to_string(), 100);

        let json = serde_json::to_string(&snapshot).unwrap();
        let deserialized: WatermarkSnapshot = serde_json::from_str(&json).unwrap();

        assert_eq!(snapshot, deserialized);
    }

    // ===========================================
    // Section 2: FileWatermarkStore Tests
    // ===========================================

    #[tokio::test]
    async fn test_file_store_load_returns_empty_when_no_file() {
        let dir = TempDir::new().unwrap();
        let store = FileWatermarkStore::new(dir.path().join("watermark.json"));

        let snapshot = store.load().await.unwrap();
        assert!(snapshot.positions.is_empty());
    }

    #[tokio::test]
    async fn test_file_store_save_and_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let store = FileWatermarkStore::new(dir.path().join("watermark.json"));

        let mut snapshot = WatermarkSnapshot::default();
        snapshot.positions.insert("store-a".to_string(), 55);
        snapshot.positions.insert("store-b".to_string(), 200);

        store.save(&snapshot).await.unwrap();
        let loaded = store.load().await.unwrap();

        assert_eq!(snapshot, loaded);
    }

    #[tokio::test]
    async fn test_file_store_overwrites_previous_data() {
        let dir = TempDir::new().unwrap();
        let store = FileWatermarkStore::new(dir.path().join("watermark.json"));

        // Save initial
        let mut first = WatermarkSnapshot::default();
        first.positions.insert("store-1".to_string(), 10);
        store.save(&first).await.unwrap();

        // Overwrite
        let mut second = WatermarkSnapshot::default();
        second.positions.insert("store-1".to_string(), 50);
        second.positions.insert("store-2".to_string(), 30);
        store.save(&second).await.unwrap();

        let loaded = store.load().await.unwrap();
        assert_eq!(loaded.positions.len(), 2);
        assert_eq!(loaded.positions["store-1"], 50);
        assert_eq!(loaded.positions["store-2"], 30);
    }

    #[tokio::test]
    async fn test_file_store_atomic_write_no_temp_file_left() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("watermark.json");
        let store = FileWatermarkStore::new(path.clone());

        let snapshot = WatermarkSnapshot::default();
        store.save(&snapshot).await.unwrap();

        // Temp file should not exist after successful save
        let tmp_path = path.with_extension("tmp");
        assert!(!tmp_path.exists());
    }

    #[tokio::test]
    async fn test_file_store_corrupted_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("watermark.json");

        // Write garbage
        tokio::fs::write(&path, b"not valid json!!!").await.unwrap();

        let store = FileWatermarkStore::new(path);
        let result = store.load().await;

        assert!(result.is_err());
    }

    // ===========================================
    // Section 3: InMemoryWatermarkStore Tests
    // ===========================================

    #[tokio::test]
    async fn test_in_memory_store_load_returns_empty() {
        let store = InMemoryWatermarkStore::new();
        let snapshot = store.load().await.unwrap();
        assert!(snapshot.positions.is_empty());
    }

    #[tokio::test]
    async fn test_in_memory_store_save_and_load() {
        let store = InMemoryWatermarkStore::new();
        let mut snapshot = WatermarkSnapshot::default();
        snapshot.positions.insert("store-1".to_string(), 42);

        store.save(&snapshot).await.unwrap();
        let loaded = store.load().await.unwrap();

        assert_eq!(loaded, snapshot);
    }
}
