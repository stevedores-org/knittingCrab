use knitting_crab_core::traits::TaskDescriptor;

/// Minimal interface for a node that the gatekeeper can evaluate.
pub trait GatekeeperNode {
    /// Returns true if this node is considered a production resource.
    fn is_production(&self) -> bool;
}

/// Enforces organizational policies on task placement across the fleet.
pub struct RosterGatekeeper;

impl RosterGatekeeper {
    /// Evaluates if a task is permitted to run on a specific node based on bullpen policies.
    pub fn can_run_on_node<N: GatekeeperNode>(task: &TaskDescriptor, node: &N) -> bool {
        // Policy: Production-promotion gates MUST run on production hardware for valid evaluation.
        if task.evaluation.is_promotion_gate && !node.is_production() {
            return false;
        }

        // Policy: Phase 1/2 agents (Local Loop / Ephemeral Hub) should be restricted to Dev nodes
        // to prevent interference with production workloads.
        if task.evaluation.agent_phase > 0
            && task.evaluation.agent_phase <= 2
            && node.is_production()
        {
            // Only allow if it's explicitly a promotion gate test.
            if !task.evaluation.is_promotion_gate {
                return false;
            }
        }

        // Policy: Phase 3/4 agents (Autonomous / Sovereign) should generally run on production.
        // But for the prototype, we prioritize the exclusion of early-phase agents from prod.

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use knitting_crab_core::ids::TaskId;
    use knitting_crab_core::traits::EvaluationMetadata;
    use std::collections::HashMap;
    use std::path::PathBuf;

    struct TestNode {
        production: bool,
    }

    impl GatekeeperNode for TestNode {
        fn is_production(&self) -> bool {
            self.production
        }
    }

    fn make_task(phase: u8, promotion_gate: bool) -> TaskDescriptor {
        TaskDescriptor {
            task_id: TaskId::new(),
            command: vec!["test".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: HashMap::new(),
            resources: Default::default(),
            policy: Default::default(),
            attempt: 0,
            is_critical: false,
            priority: knitting_crab_core::Priority::Normal,
            dependencies: vec![],
            location: Default::default(),
            evaluation: EvaluationMetadata {
                agent_phase: phase,
                is_promotion_gate: promotion_gate,
                ..Default::default()
            },
        }
    }

    #[test]
    fn phase_1_restricted_to_dev() {
        let task = make_task(1, false);
        let dev_node = TestNode { production: false };
        let prod_node = TestNode { production: true };

        assert!(RosterGatekeeper::can_run_on_node(&task, &dev_node));
        assert!(!RosterGatekeeper::can_run_on_node(&task, &prod_node));
    }

    #[test]
    fn promotion_gate_requires_prod() {
        let task = make_task(2, true);
        let dev_node = TestNode { production: false };
        let prod_node = TestNode { production: true };

        assert!(!RosterGatekeeper::can_run_on_node(&task, &dev_node));
        assert!(RosterGatekeeper::can_run_on_node(&task, &prod_node));
    }

    #[test]
    fn phase_3_can_run_on_prod() {
        let task = make_task(3, false);
        let prod_node = TestNode { production: true };

        assert!(RosterGatekeeper::can_run_on_node(&task, &prod_node));
    }
}
