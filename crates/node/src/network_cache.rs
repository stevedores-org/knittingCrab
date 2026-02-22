use crate::connection::NodeConnection;
use crate::error::NodeError;
use knitting_crab_transport::{CacheLocation, CoordinatorRequest};
use std::path::PathBuf;

pub struct NetworkCacheClient {
    conn: NodeConnection,
}

impl NetworkCacheClient {
    pub fn new(conn: NodeConnection) -> Self {
        Self { conn }
    }

    pub async fn query_locations(&self, cache_key: &str) -> Result<Vec<CacheLocation>, NodeError> {
        let req = CoordinatorRequest::QueryCache {
            cache_key: cache_key.to_string(),
        };
        match self.conn.request(req).await {
            Ok(knitting_crab_transport::CoordinatorResponse::CacheLocations(locations)) => {
                Ok(locations)
            }
            Ok(knitting_crab_transport::CoordinatorResponse::Error(e)) => Err(NodeError::Core(e)),
            Ok(_) => Err(NodeError::Core(
                "unexpected response to query cache".to_string(),
            )),
            Err(e) => Err(e),
        }
    }

    pub async fn announce(&self, cache_key: String, path: PathBuf) -> Result<(), NodeError> {
        let req = CoordinatorRequest::AnnounceCache { cache_key, path };
        match self.conn.request(req).await {
            Ok(knitting_crab_transport::CoordinatorResponse::Ok) => Ok(()),
            Ok(knitting_crab_transport::CoordinatorResponse::Error(e)) => Err(NodeError::Core(e)),
            Ok(_) => Err(NodeError::Core(
                "unexpected response to announce cache".to_string(),
            )),
            Err(e) => Err(e),
        }
    }
}
