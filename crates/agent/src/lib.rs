//! knitting-crab-agent: Agent-specific task management and goal locking.
//!
//! This crate provides utilities for orchestrating autonomous agents with guardrails:
//! - Goal lock management to prevent duplicate work on the same goal
//! - Budget tracking for token and time limits
//! - Test gate validation before task completion
//! - Agent plan generation with sequential task templates

pub mod goal_lock;

pub use goal_lock::InMemoryGoalLockStore;
