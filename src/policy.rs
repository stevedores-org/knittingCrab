use std::time::{Duration, Instant};

pub struct BackoffStrategy {
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
}

impl BackoffStrategy {
    pub fn compute_delay(&self, attempt: u32) -> Duration {
        let factor = self.multiplier.powi(attempt as i32);
        let nanos = (self.base_delay.as_nanos() as f64 * factor) as u128;
        let computed = Duration::from_nanos(nanos.min(u64::MAX as u128) as u64);
        computed.min(self.max_delay)
    }
}

pub struct Budget {
    pub max_tokens: u64,
    pub used_tokens: u64,
    pub max_duration: Duration,
    pub started_at: Instant,
}

impl Budget {
    pub fn new(max_tokens: u64, max_duration: Duration) -> Self {
        Budget {
            max_tokens,
            used_tokens: 0,
            max_duration,
            started_at: Instant::now(),
        }
    }

    pub fn is_token_exhausted(&self) -> bool {
        self.used_tokens >= self.max_tokens
    }

    pub fn is_time_exhausted(&self) -> bool {
        self.started_at.elapsed() >= self.max_duration
    }

    /// Charge tokens; returns false if the charge would exceed the budget.
    pub fn charge_tokens(&mut self, amount: u64) -> bool {
        if self.used_tokens + amount > self.max_tokens {
            return false;
        }
        self.used_tokens += amount;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_with_exponential_backoff() {
        let strategy = BackoffStrategy {
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            multiplier: 2.0,
        };
        let d0 = strategy.compute_delay(0);
        let d1 = strategy.compute_delay(1);
        let d2 = strategy.compute_delay(2);
        assert!(d1 > d0, "delay should increase with retries");
        assert!(d2 > d1, "delay should increase with retries");
    }

    #[test]
    fn backoff_capped_at_max_delay() {
        let strategy = BackoffStrategy {
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
            multiplier: 10.0,
        };
        let d = strategy.compute_delay(10);
        assert!(d <= Duration::from_millis(500));
    }

    #[test]
    fn budget_token_exhaustion() {
        let mut budget = Budget::new(10, Duration::from_secs(60));
        assert!(budget.charge_tokens(5));
        assert!(budget.charge_tokens(5));
        assert!(!budget.charge_tokens(1));
        assert!(budget.is_token_exhausted());
    }

    #[test]
    fn budget_time_exhaustion() {
        let budget = Budget {
            max_tokens: 100,
            used_tokens: 0,
            max_duration: Duration::from_millis(1),
            started_at: Instant::now() - Duration::from_secs(1),
        };
        assert!(budget.is_time_exhausted());
    }
}
