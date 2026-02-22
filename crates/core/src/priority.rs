//! Task priority levels for fair scheduling and graceful degradation.
//!
//! This module defines a 4-level priority model that enables:
//! - Priority-aware task queuing
//! - Weighted round-robin scheduling
//! - Graceful degradation under load
//! - Prevention of priority inversion
//!
//! ## Priority Levels
//!
//! ```ignore
//! Priority::Critical  (weight: 50%) - Must complete, system-critical work
//! Priority::High      (weight: 30%) - Important work with SLA requirements
//! Priority::Normal    (weight: 15%) - Default priority, general user work
//! Priority::Low       (weight: 5%)  - Background work, can be deferred
//! ```
//!
//! ## Backward Compatibility
//!
//! The `is_critical: bool` field from Phase 1 maps to priorities:
//! - `is_critical=true` → `Priority::Critical`
//! - `is_critical=false` → `Priority::Normal` (default)

use serde::{Deserialize, Serialize};
use std::fmt;

/// Task priority level with numeric backing for ordering.
///
/// Priorities determine task scheduling order and degradation behavior.
/// Higher numeric values indicate higher priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[repr(u8)]
pub enum Priority {
    /// Background work that can be deferred under load.
    /// Weight: 5% of execution time.
    Low = 0,

    /// Default priority for general user work.
    /// Weight: 15% of execution time.
    Normal = 1,

    /// Important work with SLA requirements.
    /// Weight: 30% of execution time.
    High = 2,

    /// System-critical work that must complete.
    /// Weight: 50% of execution time.
    Critical = 3,
}

impl Priority {
    /// Convert a numeric value to Priority.
    ///
    /// Returns None if value is > 3.
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Priority::Low),
            1 => Some(Priority::Normal),
            2 => Some(Priority::High),
            3 => Some(Priority::Critical),
            _ => None,
        }
    }

    /// Get the numeric value of this priority.
    pub fn as_u8(&self) -> u8 {
        *self as u8
    }

    /// Check if this priority is critical.
    ///
    /// Only Critical level is considered critical.
    pub fn is_critical(&self) -> bool {
        matches!(self, Priority::Critical)
    }

    /// Get the weight percentage for this priority in round-robin scheduling.
    ///
    /// Returns value between 0.0 and 100.0.
    ///
    /// # Example
    /// ```ignore
    /// assert_eq!(Priority::Critical.weight_percentage(), 50.0);
    /// assert_eq!(Priority::Low.weight_percentage(), 5.0);
    /// ```
    pub fn weight_percentage(&self) -> f64 {
        match self {
            Priority::Critical => 50.0,
            Priority::High => 30.0,
            Priority::Normal => 15.0,
            Priority::Low => 5.0,
        }
    }

    /// Get a human-readable label for this priority.
    pub fn label(&self) -> &'static str {
        match self {
            Priority::Critical => "Critical",
            Priority::High => "High",
            Priority::Normal => "Normal",
            Priority::Low => "Low",
        }
    }

    /// Iterate through all priority levels in ascending order.
    pub fn all() -> [Priority; 4] {
        [
            Priority::Low,
            Priority::Normal,
            Priority::High,
            Priority::Critical,
        ]
    }

    /// Iterate through all priority levels in descending order.
    pub fn all_desc() -> [Priority; 4] {
        [
            Priority::Critical,
            Priority::High,
            Priority::Normal,
            Priority::Low,
        ]
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Convert from boolean `is_critical` field (Phase 1) to Priority (Phase 2).
///
/// This enables backward compatibility:
/// - `true` (is_critical) → `Priority::Critical`
/// - `false` (not critical) → `Priority::Normal`
impl From<bool> for Priority {
    fn from(is_critical: bool) -> Self {
        if is_critical {
            Priority::Critical
        } else {
            Priority::Normal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_ordering() {
        assert!(Priority::Critical > Priority::High);
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
        assert_eq!(Priority::Critical, Priority::Critical);
    }

    #[test]
    fn test_priority_as_u8() {
        assert_eq!(Priority::Low.as_u8(), 0);
        assert_eq!(Priority::Normal.as_u8(), 1);
        assert_eq!(Priority::High.as_u8(), 2);
        assert_eq!(Priority::Critical.as_u8(), 3);
    }

    #[test]
    fn test_priority_from_u8() {
        assert_eq!(Priority::from_u8(0), Some(Priority::Low));
        assert_eq!(Priority::from_u8(1), Some(Priority::Normal));
        assert_eq!(Priority::from_u8(2), Some(Priority::High));
        assert_eq!(Priority::from_u8(3), Some(Priority::Critical));
        assert_eq!(Priority::from_u8(4), None);
        assert_eq!(Priority::from_u8(255), None);
    }

    #[test]
    fn test_priority_is_critical() {
        assert!(!Priority::Low.is_critical());
        assert!(!Priority::Normal.is_critical());
        assert!(!Priority::High.is_critical());
        assert!(Priority::Critical.is_critical());
    }

    #[test]
    fn test_priority_weights_sum_to_100() {
        let total: f64 = Priority::all().iter().map(|p| p.weight_percentage()).sum();
        assert_eq!(total, 100.0);
    }

    #[test]
    fn test_priority_weight_percentage() {
        assert_eq!(Priority::Critical.weight_percentage(), 50.0);
        assert_eq!(Priority::High.weight_percentage(), 30.0);
        assert_eq!(Priority::Normal.weight_percentage(), 15.0);
        assert_eq!(Priority::Low.weight_percentage(), 5.0);
    }

    #[test]
    fn test_priority_label() {
        assert_eq!(Priority::Critical.label(), "Critical");
        assert_eq!(Priority::High.label(), "High");
        assert_eq!(Priority::Normal.label(), "Normal");
        assert_eq!(Priority::Low.label(), "Low");
    }

    #[test]
    fn test_priority_display() {
        assert_eq!(Priority::Critical.to_string(), "Critical");
        assert_eq!(Priority::High.to_string(), "High");
        assert_eq!(Priority::Normal.to_string(), "Normal");
        assert_eq!(Priority::Low.to_string(), "Low");
    }

    #[test]
    fn test_priority_from_bool() {
        assert_eq!(Priority::from(true), Priority::Critical);
        assert_eq!(Priority::from(false), Priority::Normal);
    }

    #[test]
    fn test_priority_all_ascending() {
        let all = Priority::all();
        assert_eq!(all[0], Priority::Low);
        assert_eq!(all[1], Priority::Normal);
        assert_eq!(all[2], Priority::High);
        assert_eq!(all[3], Priority::Critical);
    }

    #[test]
    fn test_priority_all_descending() {
        let all = Priority::all_desc();
        assert_eq!(all[0], Priority::Critical);
        assert_eq!(all[1], Priority::High);
        assert_eq!(all[2], Priority::Normal);
        assert_eq!(all[3], Priority::Low);
    }

    #[test]
    fn test_priority_copy() {
        let p1 = Priority::High;
        let p2 = p1;
        assert_eq!(p1, p2);

        // Copy trait allows implicit copying without clone()
        let p3 = p1;
        assert_eq!(p1, p3);
    }

    #[test]
    fn test_priority_hash() {
        use std::collections::HashSet;

        let mut set = HashSet::new();
        set.insert(Priority::High);
        set.insert(Priority::Normal);
        set.insert(Priority::High); // Duplicate

        assert_eq!(set.len(), 2);
        assert!(set.contains(&Priority::High));
        assert!(!set.contains(&Priority::Critical));
    }
}
