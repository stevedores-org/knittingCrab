use serde::{Deserialize, Serialize};

/// Resource allocation specification (stub for Epic 1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceAllocation {
    pub cpu_cores: f32,
    pub memory_mb: u32,
}

impl ResourceAllocation {
    pub fn new(cpu_cores: f32, memory_mb: u32) -> Self {
        Self {
            cpu_cores,
            memory_mb,
        }
    }
}

impl Default for ResourceAllocation {
    fn default() -> Self {
        Self {
            cpu_cores: 1.0,
            memory_mb: 256,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_allocation() {
        let alloc = ResourceAllocation::default();
        assert_eq!(alloc.cpu_cores, 1.0);
        assert_eq!(alloc.memory_mb, 256);
    }
}
