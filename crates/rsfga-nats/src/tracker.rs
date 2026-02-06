//! Write Tracker for Read-Your-Own-Writes (RYOW) consistency.
//!
//! The `WriteTracker` tracks which write sequences have been committed to storage,
//! enabling clients to wait for their writes to be visible before reading.
//!
//! ## Architecture
//!
//! ```text
//! Client ──▶ Async Write ──▶ NATS JetStream ──▶ Storage Consumer ──▶ DB
//!    │              │                                    │
//!    │         WriteTicket                        CommittedEvent
//!    │              │                                    │
//!    └──▶ Check (with ticket) ──▶ WriteTracker.wait ◀───┘
//! ```
//!
//! ## Usage
//!
//! ```rust,no_run
//! use rsfga_nats::tracker::{WriteTracker, WriteTrackerConfig};
//! use std::time::Duration;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let tracker = WriteTracker::new(WriteTrackerConfig::default());
//!
//! // Storage consumer calls this after committing a batch
//! tracker.mark_committed("store-1", 42);
//!
//! // Read handler waits for a write to be committed
//! tracker.wait_for_commit("store-1", 42, Duration::from_secs(5)).await?;
//! # Ok(())
//! # }
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::oneshot;
use tracing::{debug, warn};

/// Error returned when a wait operation times out.
#[derive(Debug, thiserror::Error)]
#[error("write not committed within timeout")]
pub struct WriteTrackerTimeout;

/// Configuration for the `WriteTracker`.
#[derive(Debug, Clone)]
pub struct WriteTrackerConfig {
    /// Time-to-live for waiter entries before they are cleaned up.
    pub waiter_ttl: Duration,
    /// Interval for the background cleanup task.
    pub cleanup_interval: Duration,
}

impl Default for WriteTrackerConfig {
    fn default() -> Self {
        Self {
            waiter_ttl: Duration::from_secs(60),
            cleanup_interval: Duration::from_secs(30),
        }
    }
}

/// Tracks committed write sequences per store and allows readers to wait
/// for specific sequences to be committed.
///
/// Thread-safe and lock-free under normal operations via `DashMap`.
pub struct WriteTracker {
    /// Map of store_id -> last committed sequence number.
    committed: DashMap<String, Arc<AtomicU64>>,
    /// Waiters for specific (store_id, sequence) pairs.
    waiters: DashMap<(String, u64), Vec<oneshot::Sender<()>>>,
    /// Configuration.
    config: WriteTrackerConfig,
}

impl WriteTracker {
    /// Create a new `WriteTracker` with the given configuration.
    pub fn new(config: WriteTrackerConfig) -> Self {
        Self {
            committed: DashMap::new(),
            waiters: DashMap::new(),
            config,
        }
    }

    /// Mark a sequence as committed for a store.
    ///
    /// Called by the API server when it receives a `CommittedEvent` from
    /// the storage consumer (via NATS `RSFGA_EVENTS` subscription).
    ///
    /// This updates the committed sequence and wakes any waiters
    /// waiting for this or earlier sequences.
    pub fn mark_committed(&self, store_id: &str, sequence: u64) {
        // Update the committed sequence (only advance, never go backward)
        self.committed
            .entry(store_id.to_string())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .fetch_max(sequence, Ordering::SeqCst);

        debug!(
            store_id = store_id,
            sequence = sequence,
            "Marked sequence as committed"
        );

        // Wake any waiters for this or earlier sequences
        let to_wake: Vec<(String, u64)> = self
            .waiters
            .iter()
            .filter(|entry| {
                let (ref s, seq) = *entry.key();
                s == store_id && seq <= sequence
            })
            .map(|entry| entry.key().clone())
            .collect();

        for key in to_wake {
            if let Some((_, senders)) = self.waiters.remove(&key) {
                let count = senders.len();
                for sender in senders {
                    // Ignore send errors - receiver may have been dropped (timeout)
                    let _ = sender.send(());
                }
                if count > 0 {
                    debug!(
                        store_id = store_id,
                        sequence = key.1,
                        waiters = count,
                        "Woke waiters for committed sequence"
                    );
                }
            }
        }
    }

    /// Wait for a sequence to be committed for a store.
    ///
    /// Returns immediately if the sequence is already committed.
    /// Otherwise, blocks until the sequence is committed or the timeout expires.
    ///
    /// # Errors
    ///
    /// Returns `WriteTrackerTimeout` if the sequence is not committed within
    /// the specified timeout duration.
    pub async fn wait_for_commit(
        &self,
        store_id: &str,
        sequence: u64,
        timeout: Duration,
    ) -> Result<(), WriteTrackerTimeout> {
        // Fast path: check if already committed
        if let Some(committed) = self.committed.get(store_id) {
            if committed.load(Ordering::SeqCst) >= sequence {
                return Ok(());
            }
        }

        // Register a waiter
        let (tx, rx) = oneshot::channel();
        self.waiters
            .entry((store_id.to_string(), sequence))
            .or_default()
            .push(tx);

        // Check again after registering to avoid race condition:
        // The sequence may have been committed between our first check and
        // registering the waiter.
        if let Some(committed) = self.committed.get(store_id) {
            if committed.load(Ordering::SeqCst) >= sequence {
                // Already committed - remove our waiter entry if we can
                // (it may have already been consumed by mark_committed)
                return Ok(());
            }
        }

        // Wait with timeout
        tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| WriteTrackerTimeout)?
            .map_err(|_| WriteTrackerTimeout)
    }

    /// Get the last committed sequence for a store, if any.
    pub fn get_committed_sequence(&self, store_id: &str) -> Option<u64> {
        self.committed
            .get(store_id)
            .map(|v| v.load(Ordering::SeqCst))
    }

    /// Get the number of active waiters across all stores.
    pub fn active_waiter_count(&self) -> usize {
        self.waiters.iter().map(|entry| entry.value().len()).sum()
    }

    /// Get the number of tracked stores.
    pub fn tracked_store_count(&self) -> usize {
        self.committed.len()
    }

    /// Start the background cleanup task that removes expired waiters.
    ///
    /// Returns a `JoinHandle` that can be used to cancel the task.
    /// The task runs until the handle is dropped or aborted.
    pub fn start_cleanup_task(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let tracker = Arc::clone(self);
        let interval = tracker.config.cleanup_interval;
        let ttl = tracker.config.waiter_ttl;

        tokio::spawn(async move {
            let mut timer = tokio::time::interval(interval);
            timer.tick().await; // Skip first immediate tick

            loop {
                timer.tick().await;

                // Remove waiters for already-committed sequences
                let committed_keys: Vec<(String, u64)> = tracker
                    .waiters
                    .iter()
                    .filter(|entry| {
                        let (ref store_id, seq) = *entry.key();
                        tracker
                            .committed
                            .get(store_id)
                            .is_some_and(|c| c.load(Ordering::SeqCst) >= seq)
                    })
                    .map(|entry| entry.key().clone())
                    .collect();

                for key in &committed_keys {
                    if let Some((_, senders)) = tracker.waiters.remove(key) {
                        for sender in senders {
                            let _ = sender.send(());
                        }
                    }
                }

                if !committed_keys.is_empty() {
                    debug!(
                        count = committed_keys.len(),
                        "Cleaned up waiters for committed sequences"
                    );
                }

                // Log stats
                let waiter_count = tracker.active_waiter_count();
                if waiter_count > 0 {
                    debug!(
                        active_waiters = waiter_count,
                        stores = tracker.tracked_store_count(),
                        ttl_secs = ttl.as_secs(),
                        "WriteTracker cleanup stats"
                    );
                }

                // Warn if too many waiters (potential leak)
                if waiter_count > 10_000 {
                    warn!(
                        active_waiters = waiter_count,
                        "High number of active waiters - potential waiter leak"
                    );
                }
            }
        })
    }
}

impl Default for WriteTracker {
    fn default() -> Self {
        Self::new(WriteTrackerConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Test: mark_committed updates the committed sequence for a store.
    #[tokio::test]
    async fn test_tracker_marks_sequence_as_committed() {
        let tracker = WriteTracker::default();

        tracker.mark_committed("store1", 5);
        assert_eq!(tracker.get_committed_sequence("store1"), Some(5));

        // Higher sequence should update
        tracker.mark_committed("store1", 10);
        assert_eq!(tracker.get_committed_sequence("store1"), Some(10));

        // Lower sequence should NOT update (monotonic)
        tracker.mark_committed("store1", 3);
        assert_eq!(tracker.get_committed_sequence("store1"), Some(10));
    }

    /// Test: wait_for_commit returns immediately if the sequence is already committed.
    #[tokio::test]
    async fn test_tracker_wait_returns_immediately_if_committed() {
        let tracker = WriteTracker::default();

        // Pre-commit sequence 10
        tracker.mark_committed("store1", 10);

        // Wait for sequence 5 (already committed) should return immediately
        let result = tracker
            .wait_for_commit("store1", 5, Duration::from_millis(100))
            .await;
        assert!(result.is_ok());

        // Wait for sequence 10 (exactly committed) should also return immediately
        let result = tracker
            .wait_for_commit("store1", 10, Duration::from_millis(100))
            .await;
        assert!(result.is_ok());
    }

    /// Test: wait_for_commit blocks until the sequence is committed.
    #[tokio::test]
    async fn test_tracker_wait_blocks_until_committed() {
        let tracker = Arc::new(WriteTracker::default());
        let tracker_clone = Arc::clone(&tracker);

        // Spawn a task that will mark sequence 5 as committed after a short delay
        let commit_handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            tracker_clone.mark_committed("store1", 5);
        });

        // Wait for the sequence - should block until committed
        let start = std::time::Instant::now();
        let result = tracker
            .wait_for_commit("store1", 5, Duration::from_secs(5))
            .await;
        let elapsed = start.elapsed();

        assert!(result.is_ok(), "Should succeed after commit");
        assert!(
            elapsed >= Duration::from_millis(40),
            "Should have waited for commit, elapsed: {:?}",
            elapsed
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "Should not wait too long, elapsed: {:?}",
            elapsed
        );

        commit_handle.await.unwrap();
    }

    /// Test: wait_for_commit times out if the sequence is never committed.
    #[tokio::test]
    async fn test_tracker_wait_times_out() {
        let tracker = WriteTracker::default();

        let start = std::time::Instant::now();
        let result = tracker
            .wait_for_commit("store1", 999, Duration::from_millis(100))
            .await;
        let elapsed = start.elapsed();

        assert!(result.is_err(), "Should timeout");
        assert!(
            elapsed >= Duration::from_millis(90),
            "Should wait close to timeout duration, elapsed: {:?}",
            elapsed
        );
    }

    /// Test: mark_committed wakes multiple waiters for the same sequence.
    #[tokio::test]
    async fn test_tracker_wakes_multiple_waiters() {
        let tracker = Arc::new(WriteTracker::default());

        // Spawn 5 waiters for sequence 10
        let mut handles = Vec::new();
        for _ in 0..5 {
            let t = Arc::clone(&tracker);
            handles.push(tokio::spawn(async move {
                t.wait_for_commit("store1", 10, Duration::from_secs(5))
                    .await
            }));
        }

        // Short delay to ensure all waiters are registered
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Commit the sequence - should wake all 5 waiters
        tracker.mark_committed("store1", 10);

        // All waiters should succeed
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok(), "All waiters should be woken");
        }

        assert_eq!(tracker.active_waiter_count(), 0, "No waiters should remain");
    }

    /// Test: mark_committed handles out-of-order commits correctly.
    #[tokio::test]
    async fn test_tracker_handles_out_of_order_commits() {
        let tracker = Arc::new(WriteTracker::default());
        let tracker_clone = Arc::clone(&tracker);

        // Spawn a waiter for sequence 5
        let waiter = tokio::spawn(async move {
            tracker_clone
                .wait_for_commit("store1", 5, Duration::from_secs(5))
                .await
        });

        tokio::time::sleep(Duration::from_millis(20)).await;

        // Commit sequence 10 (skipping 5) - should still wake waiter for 5
        // because committed >= requested
        tracker.mark_committed("store1", 10);

        let result = waiter.await.unwrap();
        assert!(
            result.is_ok(),
            "Waiter for seq 5 should succeed when seq 10 is committed"
        );
        assert_eq!(tracker.get_committed_sequence("store1"), Some(10));
    }

    /// Test: Waiters for different stores are isolated.
    #[tokio::test]
    async fn test_tracker_per_store_isolation() {
        let tracker = Arc::new(WriteTracker::default());

        // Commit sequence 10 for store1 only
        tracker.mark_committed("store1", 10);

        // store1: sequence 5 should be immediately available
        let result = tracker
            .wait_for_commit("store1", 5, Duration::from_millis(100))
            .await;
        assert!(result.is_ok(), "store1 seq 5 should be committed");

        // store2: sequence 5 should timeout (nothing committed for store2)
        let result = tracker
            .wait_for_commit("store2", 5, Duration::from_millis(100))
            .await;
        assert!(result.is_err(), "store2 seq 5 should timeout");

        // Commit for store2 separately
        tracker.mark_committed("store2", 3);

        let result = tracker
            .wait_for_commit("store2", 3, Duration::from_millis(100))
            .await;
        assert!(result.is_ok(), "store2 seq 3 should now be committed");

        // store2 sequence 5 should still timeout
        let result = tracker
            .wait_for_commit("store2", 5, Duration::from_millis(100))
            .await;
        assert!(result.is_err(), "store2 seq 5 should still timeout");
    }

    /// Test: cleanup task removes waiters for committed sequences.
    #[tokio::test]
    async fn test_cleanup_task_removes_committed_waiters() {
        let config = WriteTrackerConfig {
            cleanup_interval: Duration::from_millis(50),
            waiter_ttl: Duration::from_secs(60),
        };
        let tracker = Arc::new(WriteTracker::new(config));

        // Register a waiter but never await it (simulates abandoned waiter)
        let (tx, _rx) = oneshot::channel();
        tracker
            .waiters
            .entry(("store1".to_string(), 5))
            .or_default()
            .push(tx);

        assert_eq!(tracker.active_waiter_count(), 1);

        // Mark sequence as committed
        tracker.mark_committed("store1", 10);

        // Start cleanup task
        let handle = tracker.start_cleanup_task();

        // Wait for cleanup to run
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Waiter should have been cleaned up
        assert_eq!(
            tracker.active_waiter_count(),
            0,
            "Cleanup should remove waiters for committed sequences"
        );

        handle.abort();
    }

    /// Test: WriteTracker stats methods work correctly.
    #[tokio::test]
    async fn test_tracker_stats() {
        let tracker = WriteTracker::default();

        assert_eq!(tracker.tracked_store_count(), 0);
        assert_eq!(tracker.active_waiter_count(), 0);
        assert_eq!(tracker.get_committed_sequence("store1"), None);

        tracker.mark_committed("store1", 5);
        tracker.mark_committed("store2", 10);

        assert_eq!(tracker.tracked_store_count(), 2);
        assert_eq!(tracker.get_committed_sequence("store1"), Some(5));
        assert_eq!(tracker.get_committed_sequence("store2"), Some(10));
    }
}
