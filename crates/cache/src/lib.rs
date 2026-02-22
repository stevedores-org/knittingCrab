//! Content-addressed artifact cache with LRU eviction and cache key generation.
//!
//! Provides mechanisms for caching build artifacts and deduplication of task outputs.

pub mod artifact;
pub mod cache_key;

pub use artifact::ArtifactCache;
pub use cache_key::CacheKeyBuilder;
