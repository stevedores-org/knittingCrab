use std::collections::{HashMap, HashSet};

use petgraph::algo::is_cyclic_directed;
use petgraph::graph::{DiGraph, NodeIndex};

use crate::error::{Result, SchedulerError};
use crate::types::WorkItem;

/// A directed acyclic graph of [`WorkItem`]s representing an execution plan.
///
/// Edges encode the dependency relationship: an edge `A → B` means *A depends
/// on B* (B must complete before A may start).
pub struct Plan {
    graph: DiGraph<WorkItem, ()>,
    node_indices: HashMap<String, NodeIndex>, // task_id → NodeIndex
}

impl Plan {
    /// Creates an empty plan.
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            node_indices: HashMap::new(),
        }
    }

    /// Inserts `item` into the plan and returns its [`NodeIndex`].
    pub fn add_task(&mut self, item: WorkItem) -> Result<NodeIndex> {
        let id = item.id.clone();
        let idx = self.graph.add_node(item);
        self.node_indices.insert(id, idx);
        Ok(idx)
    }

    /// Adds an edge expressing "`from_id` depends on `to_id`".
    ///
    /// `to_id` must complete before `from_id` may start.
    pub fn add_dependency(&mut self, from_id: &str, to_id: &str) -> Result<()> {
        let from = self
            .node_indices
            .get(from_id)
            .copied()
            .ok_or_else(|| SchedulerError::TaskNotFound(from_id.to_owned()))?;
        let to = self
            .node_indices
            .get(to_id)
            .copied()
            .ok_or_else(|| SchedulerError::TaskNotFound(to_id.to_owned()))?;
        self.graph.add_edge(from, to, ());
        Ok(())
    }

    /// Validates the plan, returning [`SchedulerError::CycleDetected`] if a
    /// cycle exists.
    pub fn validate(&self) -> Result<()> {
        if is_cyclic_directed(&self.graph) {
            return Err(SchedulerError::CycleDetected);
        }
        Ok(())
    }

    /// Returns references to tasks whose dependencies have all completed and
    /// that are not yet in the `completed` set themselves.
    pub fn ready_tasks<'a>(&'a self, completed: &HashSet<String>) -> Vec<&'a WorkItem> {
        self.graph
            .node_indices()
            .filter_map(|idx| {
                let item = &self.graph[idx];
                // Skip tasks that have already completed.
                if completed.contains(&item.id) {
                    return None;
                }
                // A task is ready when every dependency id is completed.
                let all_deps_done = item.dependencies.iter().all(|dep| completed.contains(dep));
                if all_deps_done {
                    Some(item)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Returns the total number of tasks in the plan.
    pub fn task_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Looks up a task by its string ID.
    pub fn get_task(&self, id: &str) -> Option<&WorkItem> {
        self.node_indices
            .get(id)
            .map(|&idx| &self.graph[idx])
    }
}

impl Default for Plan {
    fn default() -> Self {
        Self::new()
    }
}
