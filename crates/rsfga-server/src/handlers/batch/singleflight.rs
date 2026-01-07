//! Singleflight implementation for deduplicating concurrent batch check requests.

use dashmap::DashMap;
use tokio::sync::broadcast;

/// Result type for singleflight operations.
#[derive(Debug, Clone)]
pub struct SingleflightResult {
    pub allowed: bool,
    pub error: Option<String>,
}

/// Result of trying to acquire a singleflight slot.
pub enum SingleflightSlot {
    /// We won the race and should execute the operation.
    /// Contains the sender to broadcast results.
    Leader(broadcast::Sender<SingleflightResult>),
    /// Another task is executing; wait for its result.
    Follower(broadcast::Receiver<SingleflightResult>),
}

/// Singleflight implementation for deduplicating concurrent check requests.
///
/// When multiple requests for the same check arrive concurrently,
/// only one actual check is executed and all requesters share the result.
///
/// Uses atomic operations to prevent race conditions between checking
/// for existing requests and registering new ones.
pub struct Singleflight<K>
where
    K: std::hash::Hash + Eq + Clone,
{
    /// Map of in-flight requests to their broadcast senders.
    in_flight: DashMap<K, broadcast::Sender<SingleflightResult>>,
}

impl<K> Singleflight<K>
where
    K: std::hash::Hash + Eq + Clone,
{
    pub fn new() -> Self {
        Self {
            in_flight: DashMap::new(),
        }
    }

    /// Atomically try to acquire a slot for this operation.
    ///
    /// Returns `Leader` if this caller should execute the operation,
    /// or `Follower` if another caller is already executing it.
    ///
    /// This uses DashMap's entry API for atomic check-and-insert,
    /// preventing race conditions between try_join and register.
    pub fn acquire(&self, key: K) -> SingleflightSlot {
        use dashmap::mapref::entry::Entry;

        match self.in_flight.entry(key) {
            Entry::Occupied(entry) => {
                // Another task is already executing this operation
                SingleflightSlot::Follower(entry.get().subscribe())
            }
            Entry::Vacant(entry) => {
                // We're the first - register and become the leader
                let (tx, _rx) = broadcast::channel(1);
                entry.insert(tx.clone());
                SingleflightSlot::Leader(tx)
            }
        }
    }

    /// Remove a completed in-flight request.
    pub fn complete(&self, key: &K) {
        self.in_flight.remove(key);
    }
}

impl<K> Default for Singleflight<K>
where
    K: std::hash::Hash + Eq + Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard that ensures singleflight cleanup on drop.
///
/// This prevents resource leaks if the operation execution panics.
pub struct SingleflightGuard<'a, K>
where
    K: std::hash::Hash + Eq + Clone,
{
    singleflight: &'a Singleflight<K>,
    key: K,
    completed: bool,
}

impl<'a, K> SingleflightGuard<'a, K>
where
    K: std::hash::Hash + Eq + Clone,
{
    pub fn new(singleflight: &'a Singleflight<K>, key: K) -> Self {
        Self {
            singleflight,
            key,
            completed: false,
        }
    }

    /// Mark as completed (normal path, not panic).
    pub fn complete(mut self) {
        self.singleflight.complete(&self.key);
        self.completed = true;
    }
}

impl<K> Drop for SingleflightGuard<'_, K>
where
    K: std::hash::Hash + Eq + Clone,
{
    fn drop(&mut self) {
        // If not already completed (e.g., due to panic), clean up
        if !self.completed {
            self.singleflight.complete(&self.key);
        }
    }
}
