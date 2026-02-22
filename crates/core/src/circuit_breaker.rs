//! Circuit breaker pattern for resilience.
//!
//! Prevents cascading failures by stopping attempts to use a service when it's experiencing issues.
//! Implements the classic three-state pattern: Closed (normal) → Open (failing fast) → Half-Open (recovery test).

use crate::error::CoreError;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The state of the circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation; requests pass through.
    Closed,

    /// Service is failing; requests are rejected immediately.
    Open,

    /// Testing if service recovered; limited requests pass through.
    HalfOpen,
}

/// Configuration for circuit breaker behavior.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures needed to trip the circuit.
    pub failure_threshold: u32,

    /// Time to wait before attempting recovery from Open state.
    pub timeout: Duration,

    /// Maximum number of requests allowed in HalfOpen state.
    pub half_open_max_calls: u32,

    /// Percentage of successes needed in HalfOpen to close the circuit (0-100).
    pub half_open_success_threshold: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            timeout: Duration::from_secs(60),
            half_open_max_calls: 3,
            half_open_success_threshold: 67, // 67% success = at least 2 of 3 succeed
        }
    }
}

/// Circuit breaker for a single service/worker.
///
/// Thread-safe implementation using atomic operations for state tracking.
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,

    // State tracking
    state: Arc<parking_lot::Mutex<CircuitState>>,
    consecutive_failures: Arc<AtomicU32>,
    consecutive_successes: Arc<AtomicU32>,
    last_failure_time: Arc<AtomicU64>,

    // Half-open state tracking
    half_open_calls: Arc<AtomicU32>,
    half_open_successes: Arc<AtomicU32>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with default configuration.
    pub fn new() -> Self {
        Self::with_config(CircuitBreakerConfig::default())
    }

    /// Create a new circuit breaker with custom configuration.
    pub fn with_config(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: Arc::new(parking_lot::Mutex::new(CircuitState::Closed)),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            consecutive_successes: Arc::new(AtomicU32::new(0)),
            last_failure_time: Arc::new(AtomicU64::new(0)),
            half_open_calls: Arc::new(AtomicU32::new(0)),
            half_open_successes: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Get the current state of the circuit breaker.
    pub fn state(&self) -> CircuitState {
        let mut state = self.state.lock();
        let current_state = *state;

        // Check if we should transition from Open to HalfOpen
        if current_state == CircuitState::Open {
            if let Ok(elapsed) = self.time_since_last_failure() {
                if elapsed >= self.config.timeout {
                    *state = CircuitState::HalfOpen;
                    // Reset half-open tracking
                    self.half_open_calls.store(0, Ordering::SeqCst);
                    self.half_open_successes.store(0, Ordering::SeqCst);
                    return CircuitState::HalfOpen;
                }
            }
        }

        current_state
    }

    /// Check if the circuit is open (rejecting requests).
    pub fn is_open(&self) -> bool {
        self.state() == CircuitState::Open
    }

    /// Record a successful call.
    pub fn record_success(&self) {
        let mut state = self.state.lock();

        match *state {
            CircuitState::Closed => {
                // Reset failure count on success
                self.consecutive_failures.store(0, Ordering::SeqCst);
            }
            CircuitState::HalfOpen => {
                self.half_open_successes.fetch_add(1, Ordering::SeqCst);

                // Check if we should close the circuit
                let successes = self.half_open_successes.load(Ordering::SeqCst);
                let calls = self.half_open_calls.load(Ordering::SeqCst);

                // If we've hit max calls and have enough successes, close
                if calls >= self.config.half_open_max_calls {
                    let success_pct = (successes * 100) / calls;
                    if success_pct >= self.config.half_open_success_threshold {
                        *state = CircuitState::Closed;
                        self.consecutive_failures.store(0, Ordering::SeqCst);
                    } else {
                        // Not enough successes; go back to Open
                        *state = CircuitState::Open;
                        self.record_last_failure();
                    }
                }
            }
            CircuitState::Open => {
                // Ignore successes while Open; state must transition through HalfOpen
            }
        }
    }

    /// Record a failed call.
    pub fn record_failure(&self) {
        let mut state = self.state.lock();
        self.record_last_failure();

        match *state {
            CircuitState::Closed => {
                let failures = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;

                if failures >= self.config.failure_threshold {
                    *state = CircuitState::Open;
                }
            }
            CircuitState::HalfOpen => {
                // Single failure in half-open trips the circuit
                *state = CircuitState::Open;
                // Reset for next half-open attempt
                self.half_open_calls.store(0, Ordering::SeqCst);
                self.half_open_successes.store(0, Ordering::SeqCst);
            }
            CircuitState::Open => {
                // Already open; nothing to do
            }
        }
    }

    /// Attempt to acquire a permit to make a request.
    ///
    /// Returns Ok if the request should proceed, Err if the circuit is open.
    pub async fn call<F, T>(&self, f: F) -> Result<T, CoreError>
    where
        F: std::future::Future<Output = Result<T, CoreError>>,
    {
        // Check if we should allow this call
        let state = self.state();
        match state {
            CircuitState::Closed => {
                // Allow the call and record the outcome
                match f.await {
                    Ok(result) => {
                        self.record_success();
                        Ok(result)
                    }
                    Err(err) => {
                        self.record_failure();
                        Err(err)
                    }
                }
            }
            CircuitState::Open => Err(CoreError::Internal(
                "circuit breaker is open; service is unavailable".to_string(),
            )),
            CircuitState::HalfOpen => {
                // Check if we can make another attempt
                let calls = self.half_open_calls.fetch_add(1, Ordering::SeqCst) + 1;
                if calls > self.config.half_open_max_calls {
                    self.half_open_calls.fetch_sub(1, Ordering::SeqCst);
                    return Err(CoreError::Internal(
                        "circuit breaker half-open capacity exceeded".to_string(),
                    ));
                }

                // Allow the call and record the outcome
                match f.await {
                    Ok(result) => {
                        self.record_success();
                        Ok(result)
                    }
                    Err(err) => {
                        self.record_failure();
                        Err(err)
                    }
                }
            }
        }
    }

    /// Force reset the circuit breaker to Closed state.
    pub fn reset(&self) {
        let mut state = self.state.lock();
        *state = CircuitState::Closed;
        self.consecutive_failures.store(0, Ordering::SeqCst);
        self.consecutive_successes.store(0, Ordering::SeqCst);
        self.half_open_calls.store(0, Ordering::SeqCst);
        self.half_open_successes.store(0, Ordering::SeqCst);
    }

    /// Get statistics for monitoring and debugging.
    pub fn stats(&self) -> CircuitBreakerStats {
        CircuitBreakerStats {
            state: self.state(),
            consecutive_failures: self.consecutive_failures.load(Ordering::SeqCst),
            half_open_calls: self.half_open_calls.load(Ordering::SeqCst),
            half_open_successes: self.half_open_successes.load(Ordering::SeqCst),
        }
    }

    fn record_last_failure(&self) {
        if let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) {
            let millis = duration.as_millis() as u64;
            self.last_failure_time.store(millis, Ordering::SeqCst);
        }
    }

    fn time_since_last_failure(&self) -> Result<Duration, CoreError> {
        let last_failure_millis = self.last_failure_time.load(Ordering::SeqCst);
        if last_failure_millis == 0 {
            return Ok(Duration::from_secs(0));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| CoreError::Internal("system time error".to_string()))?;

        let now_millis = now.as_millis() as u64;
        let elapsed_millis = now_millis.saturating_sub(last_failure_millis);
        Ok(Duration::from_millis(elapsed_millis))
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for CircuitBreaker {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            state: Arc::clone(&self.state),
            consecutive_failures: Arc::clone(&self.consecutive_failures),
            consecutive_successes: Arc::clone(&self.consecutive_successes),
            last_failure_time: Arc::clone(&self.last_failure_time),
            half_open_calls: Arc::clone(&self.half_open_calls),
            half_open_successes: Arc::clone(&self.half_open_successes),
        }
    }
}

/// Statistics snapshot of the circuit breaker state.
#[derive(Debug, Clone)]
pub struct CircuitBreakerStats {
    pub state: CircuitState,
    pub consecutive_failures: u32,
    pub half_open_calls: u32,
    pub half_open_successes: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_starts_closed() {
        let cb = CircuitBreaker::new();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_opens_on_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        };
        let cb = CircuitBreaker::with_config(config);

        // Record failures
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_closed_circuit_resets_failures_on_success() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        };
        let cb = CircuitBreaker::with_config(config);

        cb.record_failure();
        cb.record_failure();
        cb.record_success();

        // Counter should reset
        assert_eq!(cb.consecutive_failures.load(Ordering::SeqCst), 0);
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_transition() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            timeout: Duration::from_millis(300), // 300ms timeout for stability
            ..Default::default()
        };
        let cb = CircuitBreaker::with_config(config);

        // Trip to Open
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Too early - should still be Open
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(cb.state(), CircuitState::Open);

        // Should transition to HalfOpen due to timeout
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[tokio::test]
    async fn test_half_open_closes_on_success() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            timeout: Duration::from_millis(50),
            half_open_max_calls: 2,
            half_open_success_threshold: 50,
        };
        let cb = CircuitBreaker::with_config(config);

        // Trip to Open
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Transition to HalfOpen
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Make successful calls - use call() to properly track half-open metrics
        let result1 = cb.call(async { Ok::<_, CoreError>(1) }).await;
        assert!(result1.is_ok());

        let result2 = cb.call(async { Ok::<_, CoreError>(2) }).await;
        assert!(result2.is_ok());

        // Should close on sufficient successes
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_half_open_reopens_on_failure() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            timeout: Duration::from_millis(50),
            half_open_max_calls: 2,
            half_open_success_threshold: 100,
        };
        let cb = CircuitBreaker::with_config(config);

        // Trip to Open
        cb.record_failure();

        // Transition to HalfOpen
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // Single failure should reopen
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[test]
    fn test_reset_clears_state() {
        let cb = CircuitBreaker::new();

        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();

        assert_eq!(cb.state(), CircuitState::Open);

        cb.reset();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_stats() {
        let cb = CircuitBreaker::new();

        cb.record_failure();
        cb.record_failure();

        let stats = cb.stats();
        assert_eq!(stats.state, CircuitState::Closed);
        assert_eq!(stats.consecutive_failures, 2);
    }

    #[tokio::test]
    async fn test_call_succeeds_in_closed_state() {
        let cb = CircuitBreaker::new();

        let result = cb.call(async { Ok::<_, CoreError>(42) }).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_call_fails_in_open_state() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            ..Default::default()
        };
        let cb = CircuitBreaker::with_config(config);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        let result = cb.call(async { Ok::<_, CoreError>(42) }).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_call_respects_half_open_max_calls() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            timeout: Duration::from_millis(50),
            half_open_max_calls: 2,
            half_open_success_threshold: 100, // Require all successes to close
        };
        let cb = CircuitBreaker::with_config(config);

        cb.record_failure();
        std::thread::sleep(Duration::from_millis(100));

        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // First two calls should succeed in half-open
        let result1 = cb.call(async { Ok::<_, CoreError>(1) }).await;
        assert!(result1.is_ok());

        let result2 = cb.call(async { Ok::<_, CoreError>(2) }).await;
        assert!(result2.is_ok());

        // Should close since we have 2/2 successes with 100% threshold
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_half_open_max_calls_with_failures() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            timeout: Duration::from_millis(50),
            half_open_max_calls: 2,
            half_open_success_threshold: 50,
        };
        let cb = CircuitBreaker::with_config(config);

        // Trip to Open then HalfOpen
        cb.record_failure();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // First call fails - circuit reopens
        let result1 = cb
            .call(async { Err::<i32, _>(CoreError::Internal("test".into())) })
            .await;
        assert!(result1.is_err());
        assert_eq!(cb.state(), CircuitState::Open);
    }
}
