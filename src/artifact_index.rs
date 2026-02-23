use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Types of artifacts that can be produced by task execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtifactType {
    /// Task execution log (stdout/stderr)
    Log,
    /// JUnit/xUnit test results
    Junit,
    /// Code coverage report
    Coverage,
    /// Patch file (diff output)
    Patch,
    /// Generic artifact
    Other,
}

impl ArtifactType {
    /// Returns the string name of the artifact type.
    pub fn as_str(&self) -> &'static str {
        match self {
            ArtifactType::Log => "log",
            ArtifactType::Junit => "junit",
            ArtifactType::Coverage => "coverage",
            ArtifactType::Patch => "patch",
            ArtifactType::Other => "other",
        }
    }

    /// Parses an artifact type from a string.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "log" => Some(ArtifactType::Log),
            "junit" => Some(ArtifactType::Junit),
            "coverage" => Some(ArtifactType::Coverage),
            "patch" => Some(ArtifactType::Patch),
            "other" => Some(ArtifactType::Other),
            _ => None,
        }
    }
}

/// Metadata about a stored artifact.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactMetadata {
    /// Unique artifact identifier
    pub id: String,
    /// Task that produced this artifact
    pub task_id: u64,
    /// Type of artifact
    pub artifact_type: ArtifactType,
    /// File size in bytes
    pub size_bytes: u64,
    /// When the artifact was created (unix timestamp)
    pub created_at: u64,
    /// Optional description or name
    pub description: Option<String>,
    /// Path where artifact is stored
    pub path: PathBuf,
}

impl ArtifactMetadata {
    /// Creates new artifact metadata with a unique ID.
    /// ID is based on task_id, artifact_type, path, and timestamp to ensure uniqueness.
    /// Even multiple artifacts of the same type from the same task will have different IDs.
    pub fn new(task_id: u64, artifact_type: ArtifactType, size_bytes: u64, path: PathBuf) -> Self {
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Generate unique ID using SHA256 hash of task_id, type, path, and timestamp
        // This ensures uniqueness even for multiple artifacts of same type from same task
        let mut hasher = Sha256::new();
        hasher.update(task_id.to_le_bytes());
        hasher.update(artifact_type.as_str().as_bytes());
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(created_at.to_le_bytes());
        let hash = format!("{:x}", hasher.finalize());
        let short_hash = &hash[..16]; // Use first 16 chars for readability

        ArtifactMetadata {
            id: format!("{}_{}{}", task_id, artifact_type.as_str(), short_hash),
            task_id,
            artifact_type,
            size_bytes,
            created_at,
            description: None,
            path,
        }
    }

    /// Sets the artifact description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

/// Retention policy for artifacts (time-based).
#[derive(Debug, Clone, Copy)]
pub struct RetentionPolicy {
    /// Maximum age of artifacts in seconds (0 = no limit)
    pub max_age_secs: u64,
}

impl RetentionPolicy {
    /// Creates a retention policy with a maximum age.
    pub fn with_max_age(max_age_secs: u64) -> Self {
        RetentionPolicy { max_age_secs }
    }

    /// Creates an unlimited retention policy.
    pub fn unlimited() -> Self {
        RetentionPolicy { max_age_secs: 0 }
    }

    /// Checks if an artifact has expired based on this policy.
    pub fn is_expired(&self, artifact: &ArtifactMetadata) -> bool {
        if self.max_age_secs == 0 {
            return false;
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let age_secs = now.saturating_sub(artifact.created_at);
        age_secs > self.max_age_secs
    }
}

/// Index of artifacts produced by tasks.
/// Allows browsing and querying artifacts by task, type, etc.
#[derive(Debug, Clone)]
pub struct ArtifactIndex {
    /// Artifacts indexed by ID
    artifacts: HashMap<String, ArtifactMetadata>,
    /// Retention policy for all artifacts
    retention: RetentionPolicy,
}

impl ArtifactIndex {
    /// Creates a new artifact index with a retention policy.
    pub fn new(retention: RetentionPolicy) -> Self {
        ArtifactIndex {
            artifacts: HashMap::new(),
            retention,
        }
    }

    /// Registers an artifact in the index.
    pub fn register(&mut self, artifact: ArtifactMetadata) {
        self.artifacts.insert(artifact.id.clone(), artifact);
    }

    /// Retrieves an artifact by ID.
    pub fn get(&self, id: &str) -> Option<&ArtifactMetadata> {
        self.artifacts.get(id)
    }

    /// Lists all artifacts for a given task.
    pub fn list_by_task(&self, task_id: u64) -> Vec<&ArtifactMetadata> {
        self.artifacts
            .values()
            .filter(|a| a.task_id == task_id && !self.retention.is_expired(a))
            .collect()
    }

    /// Lists all artifacts of a given type.
    pub fn list_by_type(&self, artifact_type: ArtifactType) -> Vec<&ArtifactMetadata> {
        self.artifacts
            .values()
            .filter(|a| a.artifact_type == artifact_type && !self.retention.is_expired(a))
            .collect()
    }

    /// Lists all artifacts for a task of a specific type.
    pub fn list_by_task_and_type(
        &self,
        task_id: u64,
        artifact_type: ArtifactType,
    ) -> Vec<&ArtifactMetadata> {
        self.artifacts
            .values()
            .filter(|a| {
                a.task_id == task_id
                    && a.artifact_type == artifact_type
                    && !self.retention.is_expired(a)
            })
            .collect()
    }

    /// Lists all non-expired artifacts in the index.
    pub fn list_all(&self) -> Vec<&ArtifactMetadata> {
        self.artifacts
            .values()
            .filter(|a| !self.retention.is_expired(a))
            .collect()
    }

    /// Removes expired artifacts from the index.
    /// Returns the number of artifacts removed.
    pub fn cleanup_expired(&mut self) -> usize {
        let before = self.artifacts.len();
        self.artifacts
            .retain(|_, artifact| !self.retention.is_expired(artifact));
        before - self.artifacts.len()
    }

    /// Gets total number of artifacts in index (including expired).
    pub fn total_artifacts(&self) -> usize {
        self.artifacts.len()
    }

    /// Gets number of non-expired artifacts.
    pub fn active_artifacts(&self) -> usize {
        self.artifacts
            .values()
            .filter(|a| !self.retention.is_expired(a))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_artifact(task_id: u64) -> ArtifactMetadata {
        ArtifactMetadata::new(
            task_id,
            ArtifactType::Log,
            1024,
            PathBuf::from(format!("/artifacts/{}_log", task_id)),
        )
    }

    #[test]
    fn artifact_type_string_conversion() {
        assert_eq!(ArtifactType::Log.as_str(), "log");
        assert_eq!(ArtifactType::Junit.as_str(), "junit");
        assert_eq!(ArtifactType::Coverage.as_str(), "coverage");
        assert_eq!(ArtifactType::Patch.as_str(), "patch");

        assert_eq!(ArtifactType::parse("log"), Some(ArtifactType::Log));
        assert_eq!(ArtifactType::parse("junit"), Some(ArtifactType::Junit));
        assert_eq!(ArtifactType::parse("invalid"), None);
    }

    #[test]
    fn artifact_metadata_creation() {
        let artifact = ArtifactMetadata::new(1, ArtifactType::Log, 2048, PathBuf::from("/path"));

        assert_eq!(artifact.task_id, 1);
        assert_eq!(artifact.artifact_type, ArtifactType::Log);
        assert_eq!(artifact.size_bytes, 2048);
        assert!(artifact.created_at > 0);
        assert_eq!(artifact.description, None);
    }

    #[test]
    fn artifact_with_description() {
        let artifact = ArtifactMetadata::new(1, ArtifactType::Log, 1024, PathBuf::from("/path"))
            .with_description("Test run log");

        assert_eq!(artifact.description, Some("Test run log".to_string()));
    }

    #[test]
    fn retention_policy_unlimited() {
        let policy = RetentionPolicy::unlimited();
        let artifact = sample_artifact(1);

        // Unlimited policy should never expire
        assert!(!policy.is_expired(&artifact));
    }

    #[test]
    fn artifact_index_registration() {
        let mut index = ArtifactIndex::new(RetentionPolicy::unlimited());
        let artifact = sample_artifact(1);

        index.register(artifact.clone());

        assert_eq!(index.total_artifacts(), 1);
        assert_eq!(index.get(&artifact.id), Some(&artifact));
    }

    #[test]
    fn index_list_by_task() {
        let mut index = ArtifactIndex::new(RetentionPolicy::unlimited());

        index.register(ArtifactMetadata::new(
            1,
            ArtifactType::Log,
            1024,
            PathBuf::from("/log"),
        ));
        index.register(ArtifactMetadata::new(
            1,
            ArtifactType::Junit,
            512,
            PathBuf::from("/junit"),
        ));
        index.register(ArtifactMetadata::new(
            2,
            ArtifactType::Log,
            1024,
            PathBuf::from("/log"),
        ));

        let task_1_artifacts = index.list_by_task(1);
        assert_eq!(task_1_artifacts.len(), 2);

        let task_2_artifacts = index.list_by_task(2);
        assert_eq!(task_2_artifacts.len(), 1);
    }

    #[test]
    fn index_list_by_type() {
        let mut index = ArtifactIndex::new(RetentionPolicy::unlimited());

        index.register(sample_artifact(1));
        index.register(ArtifactMetadata::new(
            2,
            ArtifactType::Junit,
            512,
            PathBuf::from("/junit"),
        ));
        index.register(sample_artifact(3));

        let logs = index.list_by_type(ArtifactType::Log);
        assert_eq!(logs.len(), 2);

        let junits = index.list_by_type(ArtifactType::Junit);
        assert_eq!(junits.len(), 1);
    }

    #[test]
    fn index_list_by_task_and_type() {
        let mut index = ArtifactIndex::new(RetentionPolicy::unlimited());

        index.register(ArtifactMetadata::new(
            1,
            ArtifactType::Log,
            1024,
            PathBuf::from("/log"),
        ));
        index.register(ArtifactMetadata::new(
            1,
            ArtifactType::Junit,
            512,
            PathBuf::from("/junit"),
        ));

        let task_1_logs = index.list_by_task_and_type(1, ArtifactType::Log);
        assert_eq!(task_1_logs.len(), 1);

        let task_1_junits = index.list_by_task_and_type(1, ArtifactType::Junit);
        assert_eq!(task_1_junits.len(), 1);

        let task_1_coverage = index.list_by_task_and_type(1, ArtifactType::Coverage);
        assert_eq!(task_1_coverage.len(), 0);
    }

    #[test]
    fn index_list_all() {
        let mut index = ArtifactIndex::new(RetentionPolicy::unlimited());

        for i in 0..5 {
            index.register(sample_artifact(i));
        }

        assert_eq!(index.list_all().len(), 5);
    }

    #[test]
    fn index_cleanup_with_unlimited_retention() {
        // Test cleanup with unlimited retention (should never remove)
        let mut index = ArtifactIndex::new(RetentionPolicy::unlimited());
        index.register(sample_artifact(1));
        index.register(sample_artifact(2));

        // Should never expire
        let removed = index.cleanup_expired();
        assert_eq!(removed, 0);
        assert_eq!(index.total_artifacts(), 2);
    }

    #[test]
    fn index_active_with_unlimited_retention() {
        // Test that all artifacts count as active with unlimited retention
        let mut index = ArtifactIndex::new(RetentionPolicy::unlimited());

        index.register(sample_artifact(1));
        index.register(sample_artifact(2));

        // All should be active
        assert_eq!(index.total_artifacts(), 2);
        assert_eq!(index.active_artifacts(), 2);
    }

    #[test]
    fn artifact_ids_are_unique_for_same_task_and_type() {
        // Test that multiple artifacts of the same type from the same task have unique IDs
        let artifact1 = ArtifactMetadata::new(1, ArtifactType::Log, 1024, PathBuf::from("/log1"));
        let artifact2 = ArtifactMetadata::new(1, ArtifactType::Log, 2048, PathBuf::from("/log2"));

        // IDs must be different even though task_id and type are the same
        assert_ne!(
            artifact1.id, artifact2.id,
            "Multiple artifacts of same type from same task must have unique IDs"
        );
    }

    #[test]
    fn multiple_same_type_artifacts_no_collision() {
        // Test that registering multiple artifacts of the same type doesn't cause collisions
        let mut index = ArtifactIndex::new(RetentionPolicy::unlimited());

        let artifact1 = ArtifactMetadata::new(1, ArtifactType::Log, 1024, PathBuf::from("/log1"));
        let artifact2 = ArtifactMetadata::new(1, ArtifactType::Log, 2048, PathBuf::from("/log2"));

        index.register(artifact1.clone());
        index.register(artifact2.clone());

        // Both should be stored, not overwritten
        assert_eq!(index.total_artifacts(), 2);
        assert_eq!(index.get(&artifact1.id), Some(&artifact1));
        assert_eq!(index.get(&artifact2.id), Some(&artifact2));

        // Both should appear in filtered lists
        let task_1_logs = index.list_by_task_and_type(1, ArtifactType::Log);
        assert_eq!(task_1_logs.len(), 2);
    }
}
