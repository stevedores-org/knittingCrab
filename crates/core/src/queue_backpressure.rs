//! Queue backpressure management to prevent system overload.
//!
//! Provides mechanisms to enforce queue depth limits, reject tasks under load,
//! and implement graceful degradation modes.

use crate::error::CoreError;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Degradation mode when system is under backpressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DegradationMode {
    /// Normal operation - no degradation.
    Normal,

    /// Moderate load - increase timeouts slightly, reduce parallelism.
    Moderate,

    /// High load - increase timeouts significantly, single-threaded execution.
    High,

    /// Critical load - reject non-critical tasks, only run critical work.
    Critical,
}

impl DegradationMode {
    /// Get the timeout multiplier for this degradation mode.
    pub fn timeout_multiplier(&self) -> f64 {
        match self {
            DegradationMode::Normal => 1.0,
            DegradationMode::Moderate => 1.5,
            DegradationMode::High => 2.5,
            DegradationMode::Critical => 5.0,
        }
    }

    /// Get the parallelism level (0.0 to 1.0, where 1.0 = full parallelism).
    pub fn parallelism_factor(&self) -> f64 {
        match self {
            DegradationMode::Normal => 1.0,
            DegradationMode::Moderate => 0.75,
            DegradationMode::High => 0.25,
            DegradationMode::Critical => 0.0, // Single-threaded only
        }
    }

    /// Check if this mode allows non-critical work.
    pub fn allows_non_critical(&self) -> bool {
        match self {
            DegradationMode::Normal | DegradationMode::Moderate | DegradationMode::High => true,
            DegradationMode::Critical => false,
        }
    }
}

/// Configuration for queue backpressure behavior.
#[derive(Debug, Clone)]
pub struct BackpressureConfig {
    /// Maximum queue depth before rejection.
    pub max_queue_depth: usize,

    /// Queue depth threshold for moderate degradation (percentage of max).
    pub moderate_threshold: f64, // e.g., 0.5 for 50%

    /// Queue depth threshold for high degradation (percentage of max).
    pub high_threshold: f64, // e.g., 0.8 for 80%

    /// Queue depth threshold for critical degradation (percentage of max).
    pub critical_threshold: f64, // e.g., 0.95 for 95%
}

impl Default for BackpressureConfig {
    fn default() -> Self {
        Self {
            max_queue_depth: 10_000,
            moderate_threshold: 0.5,
            high_threshold: 0.8,
            critical_threshold: 0.95,
        }
    }
}

impl BackpressureConfig {
    /// Create a strict backpressure config (lower queue limits).
    pub fn strict() -> Self {
        Self {
            max_queue_depth: 1_000,
            moderate_threshold: 0.3,
            high_threshold: 0.6,
            critical_threshold: 0.85,
        }
    }

    /// Create a permissive backpressure config (higher queue limits).
    pub fn permissive() -> Self {
        Self {
            max_queue_depth: 50_000,
            moderate_threshold: 0.7,
            high_threshold: 0.9,
            critical_threshold: 0.98,
        }
    }

    /// Get the actual queue depth at which degradation mode changes.
    pub fn moderate_depth(&self) -> usize {
        (self.max_queue_depth as f64 * self.moderate_threshold) as usize
    }

    pub fn high_depth(&self) -> usize {
        (self.max_queue_depth as f64 * self.high_threshold) as usize
    }

    pub fn critical_depth(&self) -> usize {
        (self.max_queue_depth as f64 * self.critical_threshold) as usize
    }
}

/// Queue backpressure manager that tracks queue depth and enforces limits.
pub struct QueueBackpressureManager {
    config: BackpressureConfig,
    current_depth: Arc<AtomicUsize>,
}

impl QueueBackpressureManager {
    /// Create a new backpressure manager.
    pub fn new(config: BackpressureConfig) -> Self {
        Self {
            config,
            current_depth: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Attempt to add an item to the queue.
    ///
    /// Returns Ok if the item can be queued, Err if queue is full.
    pub fn try_enqueue(&self) -> Result<(), CoreError> {
        let current = self.current_depth.load(Ordering::SeqCst);

        if current >= self.config.max_queue_depth {
            return Err(CoreError::Internal(
                "queue is full; backpressure limit exceeded".to_string(),
            ));
        }

        self.current_depth.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Add an item to the queue without checking limits (for internal use).
    pub fn enqueue(&self) {
        self.current_depth.fetch_add(1, Ordering::SeqCst);
    }

    /// Remove an item from the queue.
    pub fn dequeue(&self) {
        self.current_depth.fetch_sub(1, Ordering::SeqCst);
    }

    /// Get the current queue depth.
    pub fn queue_depth(&self) -> usize {
        self.current_depth.load(Ordering::SeqCst)
    }

    /// Get the current degradation mode based on queue depth.
    pub fn degradation_mode(&self) -> DegradationMode {
        let depth = self.queue_depth();

        if depth >= self.config.critical_depth() {
            DegradationMode::Critical
        } else if depth >= self.config.high_depth() {
            DegradationMode::High
        } else if depth >= self.config.moderate_depth() {
            DegradationMode::Moderate
        } else {
            DegradationMode::Normal
        }
    }

    /// Get the queue utilization as a percentage (0.0 to 1.0).
    pub fn utilization(&self) -> f64 {
        let depth = self.queue_depth() as f64;
        let max = self.config.max_queue_depth as f64;
        (depth / max).min(1.0)
    }

    /// Get stats about the queue state.
    pub fn stats(&self) -> QueueStats {
        let depth = self.queue_depth();
        QueueStats {
            current_depth: depth,
            max_depth: self.config.max_queue_depth,
            utilization: self.utilization(),
            degradation_mode: self.degradation_mode(),
        }
    }

    /// Reset the queue depth (for testing or recovery).
    pub fn reset(&self) {
        self.current_depth.store(0, Ordering::SeqCst);
    }

    /// Clone the depth counter for shared access.
    pub fn clone_depth(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.current_depth)
    }
}

impl Clone for QueueBackpressureManager {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            current_depth: Arc::clone(&self.current_depth),
        }
    }
}

/// Statistics snapshot of queue state.
#[derive(Debug, Clone)]
pub struct QueueStats {
    pub current_depth: usize,
    pub max_depth: usize,
    pub utilization: f64,
    pub degradation_mode: DegradationMode,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_degradation_mode_timeout_multiplier() {
        assert_eq!(DegradationMode::Normal.timeout_multiplier(), 1.0);
        assert_eq!(DegradationMode::Moderate.timeout_multiplier(), 1.5);
        assert_eq!(DegradationMode::High.timeout_multiplier(), 2.5);
        assert_eq!(DegradationMode::Critical.timeout_multiplier(), 5.0);
    }

    #[test]
    fn test_degradation_mode_parallelism() {
        assert_eq!(DegradationMode::Normal.parallelism_factor(), 1.0);
        assert_eq!(DegradationMode::Moderate.parallelism_factor(), 0.75);
        assert_eq!(DegradationMode::High.parallelism_factor(), 0.25);
        assert_eq!(DegradationMode::Critical.parallelism_factor(), 0.0);
    }

    #[test]
    fn test_degradation_mode_allows_non_critical() {
        assert!(DegradationMode::Normal.allows_non_critical());
        assert!(DegradationMode::Moderate.allows_non_critical());
        assert!(DegradationMode::High.allows_non_critical());
        assert!(!DegradationMode::Critical.allows_non_critical());
    }

    #[test]
    fn test_backpressure_config_default() {
        let config = BackpressureConfig::default();
        assert_eq!(config.max_queue_depth, 10_000);
        assert_eq!(config.moderate_depth(), 5_000);
        assert_eq!(config.high_depth(), 8_000);
        assert_eq!(config.critical_depth(), 9_500);
    }

    #[test]
    fn test_backpressure_config_strict() {
        let config = BackpressureConfig::strict();
        assert_eq!(config.max_queue_depth, 1_000);
        assert_eq!(config.moderate_depth(), 300);
        assert_eq!(config.high_depth(), 600);
        assert_eq!(config.critical_depth(), 850);
    }

    #[test]
    fn test_backpressure_config_permissive() {
        let config = BackpressureConfig::permissive();
        assert_eq!(config.max_queue_depth, 50_000);
        assert_eq!(config.moderate_depth(), 35_000);
        assert_eq!(config.high_depth(), 45_000);
        assert_eq!(config.critical_depth(), 49_000);
    }

    #[test]
    fn test_queue_backpressure_enqueue_dequeue() {
        let manager = QueueBackpressureManager::new(BackpressureConfig::default());

        assert_eq!(manager.queue_depth(), 0);
        manager.enqueue();
        assert_eq!(manager.queue_depth(), 1);

        manager.dequeue();
        assert_eq!(manager.queue_depth(), 0);
    }

    #[test]
    fn test_queue_backpressure_try_enqueue_success() {
        let manager = QueueBackpressureManager::new(BackpressureConfig::default());

        for _ in 0..100 {
            assert!(manager.try_enqueue().is_ok());
        }

        assert_eq!(manager.queue_depth(), 100);
    }

    #[test]
    fn test_queue_backpressure_try_enqueue_rejects_when_full() {
        let config = BackpressureConfig {
            max_queue_depth: 10,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);

        // Fill the queue
        for _ in 0..10 {
            assert!(manager.try_enqueue().is_ok());
        }

        // Next enqueue should fail
        assert!(manager.try_enqueue().is_err());
    }

    #[test]
    fn test_degradation_mode_detection() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            moderate_threshold: 0.5,
            high_threshold: 0.8,
            critical_threshold: 0.95,
        };
        let manager = QueueBackpressureManager::new(config);

        // Normal
        assert_eq!(manager.degradation_mode(), DegradationMode::Normal);

        // Moderate
        for _ in 0..50 {
            manager.enqueue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::Moderate);

        // High
        for _ in 0..30 {
            manager.enqueue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::High);

        // Critical
        for _ in 0..15 {
            manager.enqueue();
        }
        assert_eq!(manager.degradation_mode(), DegradationMode::Critical);
    }

    #[test]
    fn test_queue_utilization() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);

        assert_eq!(manager.utilization(), 0.0);

        for _ in 0..50 {
            manager.enqueue();
        }
        assert_eq!(manager.utilization(), 0.5);

        for _ in 0..50 {
            manager.enqueue();
        }
        assert_eq!(manager.utilization(), 1.0);
    }

    #[test]
    fn test_queue_stats() {
        let config = BackpressureConfig {
            max_queue_depth: 100,
            ..Default::default()
        };
        let manager = QueueBackpressureManager::new(config);

        for _ in 0..25 {
            manager.enqueue();
        }

        let stats = manager.stats();
        assert_eq!(stats.current_depth, 25);
        assert_eq!(stats.max_depth, 100);
        assert_eq!(stats.utilization, 0.25);
    }

    #[test]
    fn test_queue_reset() {
        let manager = QueueBackpressureManager::new(BackpressureConfig::default());

        for _ in 0..100 {
            manager.enqueue();
        }

        assert_eq!(manager.queue_depth(), 100);
        manager.reset();
        assert_eq!(manager.queue_depth(), 0);
    }

    #[test]
    fn test_queue_clone() {
        let manager = QueueBackpressureManager::new(BackpressureConfig::default());

        for _ in 0..50 {
            manager.enqueue();
        }

        let cloned = manager.clone();
        assert_eq!(cloned.queue_depth(), 50);

        cloned.enqueue();
        assert_eq!(manager.queue_depth(), 51); // Shares state
    }
}
