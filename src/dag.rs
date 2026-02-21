use crate::error::SchedulerError;
use crate::work_item::{TaskState, WorkItem};
use std::collections::HashMap;

pub struct DagEngine {
    tasks: HashMap<u64, WorkItem>,
}

impl DagEngine {
    pub fn new() -> Self {
        DagEngine {
            tasks: HashMap::new(),
        }
    }

    pub fn add_task(&mut self, item: WorkItem) -> Result<(), SchedulerError> {
        let id = item.id;
        self.tasks.insert(id, item);
        if self.has_cycle() {
            self.tasks.remove(&id);
            return Err(SchedulerError::CycleDetected { task_id: id });
        }
        Ok(())
    }

    pub fn remove_task(&mut self, id: u64) {
        self.tasks.remove(&id);
    }

    pub fn ready_tasks(&self) -> Vec<u64> {
        self.tasks
            .values()
            .filter(|t| {
                matches!(t.state, TaskState::Pending | TaskState::Scheduled)
                    && t.dependencies.iter().all(|dep_id| {
                        self.tasks
                            .get(dep_id)
                            .map(|d| d.state == TaskState::Success)
                            .unwrap_or(true) // If dep not tracked, consider it satisfied
                    })
            })
            .map(|t| t.id)
            .collect()
    }

    pub fn mark_success(&mut self, id: u64) {
        if let Some(task) = self.tasks.get_mut(&id) {
            task.state = TaskState::Success;
        }
    }

    pub fn mark_failed(&mut self, id: u64, reason: String) {
        if let Some(task) = self.tasks.get_mut(&id) {
            task.state = TaskState::Failed(reason);
        }
    }

    pub fn has_cycle(&self) -> bool {
        // DFS cycle detection using three-color marking (White/Gray/Black)
        enum Color {
            White,
            Gray,
            Black,
        }
        let mut colors: HashMap<u64, Color> =
            self.tasks.keys().map(|&id| (id, Color::White)).collect();

        fn dfs(id: u64, tasks: &HashMap<u64, WorkItem>, colors: &mut HashMap<u64, Color>) -> bool {
            colors.insert(id, Color::Gray);
            if let Some(task) = tasks.get(&id) {
                for &dep in &task.dependencies {
                    if !tasks.contains_key(&dep) {
                        continue;
                    }
                    match colors.get(&dep) {
                        Some(Color::Gray) => return true,
                        Some(Color::White) => {
                            if dfs(dep, tasks, colors) {
                                return true;
                            }
                        }
                        _ => {}
                    }
                }
            }
            colors.insert(id, Color::Black);
            false
        }

        let ids: Vec<u64> = self.tasks.keys().copied().collect();
        for id in ids {
            if matches!(colors.get(&id), Some(Color::White)) && dfs(id, &self.tasks, &mut colors) {
                return true;
            }
        }
        false
    }

    pub fn get_task(&self, id: u64) -> Option<&WorkItem> {
        self.tasks.get(&id)
    }

    pub fn get_task_mut(&mut self, id: u64) -> Option<&mut WorkItem> {
        self.tasks.get_mut(&id)
    }
}

impl Default for DagEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_item::Priority;

    fn make_task(id: u64, deps: Vec<u64>) -> WorkItem {
        WorkItem::new(
            id,
            format!("goal{}", id),
            "repo",
            "main",
            Priority::Batch,
            deps,
            1,
            512,
            false,
            None,
            0,
        )
    }

    #[test]
    fn cannot_run_before_deps_complete() {
        let mut dag = DagEngine::new();
        dag.add_task(make_task(1, vec![])).unwrap();
        dag.add_task(make_task(2, vec![1])).unwrap();

        let ready = dag.ready_tasks();
        assert!(ready.contains(&1));
        assert!(!ready.contains(&2));
    }

    #[test]
    fn dep_complete_unblocks_child() {
        let mut dag = DagEngine::new();
        dag.add_task(make_task(1, vec![])).unwrap();
        dag.add_task(make_task(2, vec![1])).unwrap();

        dag.mark_success(1);
        let ready = dag.ready_tasks();
        assert!(ready.contains(&2));
    }

    #[test]
    fn cycles_are_rejected() {
        let mut dag = DagEngine::new();
        dag.add_task(make_task(1, vec![2])).unwrap();
        let result = dag.add_task(make_task(2, vec![1]));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SchedulerError::CycleDetected { task_id: 2 }
        ));
    }

    #[test]
    fn partial_failure_propagates() {
        let mut dag = DagEngine::new();
        dag.add_task(make_task(1, vec![])).unwrap();
        dag.add_task(make_task(2, vec![1])).unwrap();

        dag.mark_failed(1, "something went wrong".into());
        let ready = dag.ready_tasks();
        assert!(!ready.contains(&2));
    }
}
