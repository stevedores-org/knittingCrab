//! Time-slice scheduler for weighted round-robin task scheduling.
//!
//! Implements a deterministic round-robin scheduler that allocates execution time
//! proportionally to priority levels, preventing starvation while prioritizing critical work.
//!
//! ## Scheduling Algorithm
//!
//! The scheduler maintains a round-robin queue of priorities with weights:
//! - Critical: 50% (appears 50 times per 100 cycles)
//! - High: 30% (appears 30 times per 100 cycles)
//! - Normal: 15% (appears 15 times per 100 cycles)
//! - Low: 5% (appears 5 times per 100 cycles)
//!
//! Each call to `next_priority()` advances the state and returns the next priority to service.

use crate::priority::Priority;
use std::collections::VecDeque;

/// Scheduler state for weighted round-robin execution.
///
/// Maintains deterministic position in the round-robin sequence.
#[derive(Debug, Clone)]
pub struct SchedulerState {
    /// Queue of priorities in round-robin order.
    queue: VecDeque<Priority>,
    /// Current position in the queue.
    current_index: usize,
    /// Number of complete rounds executed.
    round_counter: u64,
}

/// Stateless weighted round-robin scheduler for priority-based task execution.
///
/// Provides deterministic scheduling that ensures all priority levels eventually
/// execute while giving higher priority tasks more frequent turns.
pub struct TimeSliceScheduler;

impl TimeSliceScheduler {
    /// Weight percentage for Critical priority tasks (50%)
    pub const CRITICAL_WEIGHT: usize = 50;
    /// Weight percentage for High priority tasks (30%)
    pub const HIGH_WEIGHT: usize = 30;
    /// Weight percentage for Normal priority tasks (15%)
    pub const NORMAL_WEIGHT: usize = 15;
    /// Weight percentage for Low priority tasks (5%)
    pub const LOW_WEIGHT: usize = 5;

    /// Get the weight percentage for a specific priority.
    ///
    /// # Example
    /// ```ignore
    /// assert_eq!(TimeSliceScheduler::weight_for_priority(Priority::Critical), 50.0);
    /// assert_eq!(TimeSliceScheduler::weight_for_priority(Priority::Low), 5.0);
    /// ```
    pub fn weight_for_priority(priority: Priority) -> f64 {
        priority.weight_percentage()
    }

    /// Create a fresh scheduler state for a new round.
    ///
    /// The state is initialized with a round-robin queue expanded to 100 entries
    /// (to represent percentages), allowing deterministic cycling through priorities.
    pub fn new_state() -> SchedulerState {
        let mut queue = VecDeque::with_capacity(100);

        // Build queue with priorities repeated according to their weights
        for _ in 0..Self::CRITICAL_WEIGHT {
            queue.push_back(Priority::Critical);
        }
        for _ in 0..Self::HIGH_WEIGHT {
            queue.push_back(Priority::High);
        }
        for _ in 0..Self::NORMAL_WEIGHT {
            queue.push_back(Priority::Normal);
        }
        for _ in 0..Self::LOW_WEIGHT {
            queue.push_back(Priority::Low);
        }

        SchedulerState {
            queue,
            current_index: 0,
            round_counter: 0,
        }
    }

    /// Get the next priority to service based on scheduler state.
    ///
    /// Deterministically cycles through the weighted round-robin queue.
    /// Returns the priority and advances the state.
    ///
    /// # Example
    /// ```ignore
    /// let mut state = TimeSliceScheduler::new_state();
    /// let p1 = TimeSliceScheduler::next_priority(&mut state); // Critical (first 50)
    /// let p2 = TimeSliceScheduler::next_priority(&mut state); // Critical
    /// // ... more Critical turns ...
    /// let p51 = TimeSliceScheduler::next_priority(&mut state); // High (next 30)
    /// ```
    pub fn next_priority(state: &mut SchedulerState) -> Priority {
        if state.queue.is_empty() {
            return Priority::Normal; // Fallback (shouldn't happen)
        }

        let priority = state.queue[state.current_index];

        // Advance to next position
        state.current_index = (state.current_index + 1) % state.queue.len();

        // Track when we complete a full round
        if state.current_index == 0 {
            state.round_counter += 1;
        }

        priority
    }

    /// Reset scheduler state to beginning of a new round.
    pub fn reset_state(state: &mut SchedulerState) {
        state.current_index = 0;
        state.round_counter += 1;
    }

    /// Get the current round number.
    pub fn round_number(state: &SchedulerState) -> u64 {
        state.round_counter
    }

    /// Get the current position in the round.
    pub fn current_position(state: &SchedulerState) -> usize {
        state.current_index
    }

    /// Get the priority distribution statistics.
    pub fn stats(state: &SchedulerState) -> SchedulerStats {
        SchedulerStats {
            round_number: state.round_counter,
            current_position: state.current_index,
            total_positions: state.queue.len(),
            critical_count: Self::CRITICAL_WEIGHT,
            high_count: Self::HIGH_WEIGHT,
            normal_count: Self::NORMAL_WEIGHT,
            low_count: Self::LOW_WEIGHT,
        }
    }
}

/// Statistics about scheduler state.
#[derive(Debug, Clone)]
pub struct SchedulerStats {
    /// Current round number (increments after each full cycle).
    pub round_number: u64,
    /// Current position within the round (0 to queue length).
    pub current_position: usize,
    /// Total positions in one complete round.
    pub total_positions: usize,
    /// Number of Critical priority slots.
    pub critical_count: usize,
    /// Number of High priority slots.
    pub high_count: usize,
    /// Number of Normal priority slots.
    pub normal_count: usize,
    /// Number of Low priority slots.
    pub low_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_state_initialization() {
        let state = TimeSliceScheduler::new_state();
        assert_eq!(state.current_index, 0);
        assert_eq!(state.round_counter, 0);
        assert_eq!(state.queue.len(), 100);
    }

    #[test]
    fn test_next_priority_cycles_correctly() {
        let mut state = TimeSliceScheduler::new_state();

        // First 50 should be Critical
        for i in 0..50 {
            let p = TimeSliceScheduler::next_priority(&mut state);
            assert_eq!(p, Priority::Critical, "position {}", i);
        }

        // Next 30 should be High
        for i in 0..30 {
            let p = TimeSliceScheduler::next_priority(&mut state);
            assert_eq!(p, Priority::High, "position {}", 50 + i);
        }

        // Next 15 should be Normal
        for i in 0..15 {
            let p = TimeSliceScheduler::next_priority(&mut state);
            assert_eq!(p, Priority::Normal, "position {}", 80 + i);
        }

        // Final 5 should be Low
        for i in 0..5 {
            let p = TimeSliceScheduler::next_priority(&mut state);
            assert_eq!(p, Priority::Low, "position {}", 95 + i);
        }

        // After 100 calls, should wrap around
        assert_eq!(state.current_index, 0);
        assert_eq!(state.round_counter, 1);
    }

    #[test]
    fn test_weight_distribution() {
        let mut state = TimeSliceScheduler::new_state();

        let mut critical_count = 0;
        let mut high_count = 0;
        let mut normal_count = 0;
        let mut low_count = 0;

        // Run 100 iterations (one complete round)
        for _ in 0..100 {
            match TimeSliceScheduler::next_priority(&mut state) {
                Priority::Critical => critical_count += 1,
                Priority::High => high_count += 1,
                Priority::Normal => normal_count += 1,
                Priority::Low => low_count += 1,
            }
        }

        assert_eq!(critical_count, 50);
        assert_eq!(high_count, 30);
        assert_eq!(normal_count, 15);
        assert_eq!(low_count, 5);
    }

    #[test]
    fn test_round_counter_increments() {
        let mut state = TimeSliceScheduler::new_state();

        assert_eq!(TimeSliceScheduler::round_number(&state), 0);

        // Run 100 iterations
        for _ in 0..100 {
            TimeSliceScheduler::next_priority(&mut state);
        }
        assert_eq!(TimeSliceScheduler::round_number(&state), 1);

        // Run 100 more
        for _ in 0..100 {
            TimeSliceScheduler::next_priority(&mut state);
        }
        assert_eq!(TimeSliceScheduler::round_number(&state), 2);
    }

    #[test]
    fn test_reset_state() {
        let mut state = TimeSliceScheduler::new_state();

        // Advance some iterations
        for _ in 0..30 {
            TimeSliceScheduler::next_priority(&mut state);
        }
        assert_eq!(state.current_index, 30);

        // Reset
        TimeSliceScheduler::reset_state(&mut state);
        assert_eq!(state.current_index, 0);
        assert_eq!(state.round_counter, 1);
    }

    #[test]
    fn test_weight_for_priority() {
        assert_eq!(
            TimeSliceScheduler::weight_for_priority(Priority::Critical),
            50.0
        );
        assert_eq!(
            TimeSliceScheduler::weight_for_priority(Priority::High),
            30.0
        );
        assert_eq!(
            TimeSliceScheduler::weight_for_priority(Priority::Normal),
            15.0
        );
        assert_eq!(TimeSliceScheduler::weight_for_priority(Priority::Low), 5.0);
    }

    #[test]
    fn test_current_position() {
        let mut state = TimeSliceScheduler::new_state();

        for i in 0..25 {
            assert_eq!(TimeSliceScheduler::current_position(&state), i);
            TimeSliceScheduler::next_priority(&mut state);
        }
    }

    #[test]
    fn test_stats() {
        let mut state = TimeSliceScheduler::new_state();

        let stats = TimeSliceScheduler::stats(&state);
        assert_eq!(stats.round_number, 0);
        assert_eq!(stats.current_position, 0);
        assert_eq!(stats.total_positions, 100);
        assert_eq!(stats.critical_count, 50);
        assert_eq!(stats.high_count, 30);
        assert_eq!(stats.normal_count, 15);
        assert_eq!(stats.low_count, 5);

        // Advance and check again
        for _ in 0..50 {
            TimeSliceScheduler::next_priority(&mut state);
        }

        let stats = TimeSliceScheduler::stats(&state);
        assert_eq!(stats.current_position, 50);
        assert_eq!(stats.round_number, 0); // Not yet complete
    }

    #[test]
    fn test_starvation_prevention() {
        let mut state = TimeSliceScheduler::new_state();

        // Run 1000 iterations (10 complete rounds)
        let mut priorities_seen = [0, 0, 0, 0];
        for _ in 0..1000 {
            let p = TimeSliceScheduler::next_priority(&mut state);
            priorities_seen[p.as_u8() as usize] += 1;
        }

        // Each priority should be serviced proportionally
        // Over 1000 calls: Critical ~500, High ~300, Normal ~150, Low ~50
        assert!(priorities_seen[3] >= 450 && priorities_seen[3] <= 550); // Critical
        assert!(priorities_seen[2] >= 250 && priorities_seen[2] <= 350); // High
        assert!(priorities_seen[1] >= 100 && priorities_seen[1] <= 200); // Normal
        assert!(priorities_seen[0] >= 0 && priorities_seen[0] <= 100); // Low (0-5% variance allowed)
    }

    #[test]
    fn test_deterministic_ordering() {
        let mut state1 = TimeSliceScheduler::new_state();
        let mut state2 = TimeSliceScheduler::new_state();

        // Both states should produce identical sequences
        for _ in 0..100 {
            let p1 = TimeSliceScheduler::next_priority(&mut state1);
            let p2 = TimeSliceScheduler::next_priority(&mut state2);
            assert_eq!(p1, p2, "states diverged");
        }
    }

    #[test]
    fn test_no_low_priority_starvation() {
        let mut state = TimeSliceScheduler::new_state();

        let mut low_seen = false;

        // Low priority should appear within first 100 calls
        for _ in 0..100 {
            if TimeSliceScheduler::next_priority(&mut state) == Priority::Low {
                low_seen = true;
                break;
            }
        }

        assert!(low_seen, "Low priority tasks would starve");
    }
}
