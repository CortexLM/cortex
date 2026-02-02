//! Thread spawn guards for limiting multi-agent capabilities.

use super::ThreadId;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Mutex;

/// Maximum depth for thread spawning (0 = initial, 1 = first child).
pub const MAX_THREAD_SPAWN_DEPTH: i32 = 1;

/// Default maximum number of concurrent agent threads.
pub const DEFAULT_MAX_THREADS: usize = 10;

/// Guards for multi-agent spawn limits per user session.
pub struct Guards {
    /// Set of active thread IDs.
    threads_set: Mutex<HashSet<ThreadId>>,

    /// Total count of spawned threads (includes completed).
    total_count: AtomicUsize,

    /// Count of pending reservations (not yet committed).
    pending_count: AtomicUsize,

    /// Maximum allowed concurrent threads.
    max_threads: usize,
}

impl Guards {
    /// Create new guards with default limits.
    pub fn new() -> Self {
        Self {
            threads_set: Mutex::new(HashSet::new()),
            total_count: AtomicUsize::new(0),
            pending_count: AtomicUsize::new(0),
            max_threads: DEFAULT_MAX_THREADS,
        }
    }

    /// Create guards with custom max threads limit.
    pub fn with_max_threads(max_threads: usize) -> Self {
        Self {
            threads_set: Mutex::new(HashSet::new()),
            total_count: AtomicUsize::new(0),
            pending_count: AtomicUsize::new(0),
            max_threads,
        }
    }

    /// Reserve a spawn slot, returning a RAII guard.
    /// Returns None if limit is exceeded.
    pub async fn reserve_spawn_slot(self: &Arc<Self>) -> Option<SpawnReservation> {
        let threads = self.threads_set.lock().await;
        let current_count = threads.len();
        let pending = self.pending_count.load(Ordering::Acquire);

        if current_count + pending >= self.max_threads {
            return None;
        }

        // Increment pending and total count
        self.pending_count.fetch_add(1, Ordering::AcqRel);
        self.total_count.fetch_add(1, Ordering::AcqRel);

        Some(SpawnReservation {
            state: Arc::clone(self),
            active: true,
        })
    }

    /// Register a spawned thread after successful spawn.
    pub(crate) async fn register_spawned_thread(&self, thread_id: ThreadId) {
        let mut threads = self.threads_set.lock().await;
        threads.insert(thread_id);
    }

    /// Release a spawned thread when it completes.
    pub async fn release_spawned_thread(&self, thread_id: ThreadId) {
        let mut threads = self.threads_set.lock().await;
        threads.remove(&thread_id);
    }

    /// Get current number of active threads.
    pub async fn active_count(&self) -> usize {
        let threads = self.threads_set.lock().await;
        threads.len()
    }

    /// Get total number of threads spawned (includes completed).
    pub fn total_spawned(&self) -> usize {
        self.total_count.load(Ordering::Acquire)
    }

    /// Get the maximum threads limit.
    pub fn max_threads(&self) -> usize {
        self.max_threads
    }
}

impl Default for Guards {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard for spawn slot reservation.
/// If not committed, the slot is released on drop.
pub struct SpawnReservation {
    state: Arc<Guards>,
    active: bool,
}

impl SpawnReservation {
    /// Commit the reservation with the actual thread ID.
    /// This transfers ownership of the slot to the spawned thread.
    pub async fn commit(mut self, thread_id: ThreadId) {
        self.state.register_spawned_thread(thread_id).await;
        // Decrement pending count since it's now committed
        self.state.pending_count.fetch_sub(1, Ordering::AcqRel);
        self.active = false;
    }

    /// Cancel the reservation without spawning.
    pub fn cancel(mut self) {
        if self.active {
            self.state.pending_count.fetch_sub(1, Ordering::AcqRel);
            self.state.total_count.fetch_sub(1, Ordering::AcqRel);
            self.active = false;
        }
    }
}

impl Drop for SpawnReservation {
    fn drop(&mut self) {
        if self.active {
            // If not committed, decrement the counts
            self.state.pending_count.fetch_sub(1, Ordering::AcqRel);
            self.state.total_count.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Check if a depth exceeds the thread spawn limit.
pub fn exceeds_thread_spawn_depth_limit(depth: i32) -> bool {
    depth > MAX_THREAD_SPAWN_DEPTH
}

/// Get the next spawn depth from current depth.
pub fn next_spawn_depth(current_depth: i32) -> i32 {
    current_depth + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_guards_reservation() {
        let guards = Arc::new(Guards::with_max_threads(2));

        // Reserve first slot
        let res1 = guards.reserve_spawn_slot().await;
        assert!(res1.is_some());

        // Reserve second slot
        let res2 = guards.reserve_spawn_slot().await;
        assert!(res2.is_some());

        // Third should fail
        let res3 = guards.reserve_spawn_slot().await;
        assert!(res3.is_none());

        // Commit first reservation
        let thread_id = ThreadId::new();
        res1.unwrap().commit(thread_id).await;

        assert_eq!(guards.active_count().await, 1);

        // Release the thread
        guards.release_spawned_thread(thread_id).await;
        assert_eq!(guards.active_count().await, 0);
    }

    #[test]
    fn test_depth_limit() {
        assert!(!exceeds_thread_spawn_depth_limit(0));
        assert!(!exceeds_thread_spawn_depth_limit(1));
        assert!(exceeds_thread_spawn_depth_limit(2));
        assert!(exceeds_thread_spawn_depth_limit(5));
    }
}
