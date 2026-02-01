//! Beacon API blob fetcher with LRU caching.

use std::sync::Arc;

use alloy_eips::eip4844::{kzg_to_versioned_hash, Blob};
use alloy_primitives::B256;
use alloy_rpc_types_beacon::sidecar::BeaconBlobBundle;
use reqwest::Client;
use schnellru::{ByLength, LruMap};
use tokio::sync::RwLock;

use super::PipelineError;

/// Cache key for blobs: (slot, versioned_hash).
type BlobCacheKey = (u64, B256);

/// Fetches blobs from the Beacon API with LRU caching.
pub struct BlobFetcher {
    client: Client,
    beacon_url: String,
    cache: Arc<RwLock<LruMap<BlobCacheKey, Box<Blob>, ByLength>>>,
}

impl BlobFetcher {
    /// Create a new blob fetcher.
    ///
    /// # Arguments
    /// * `beacon_url` - Base URL for the Beacon API (e.g., "https://beacon.example.com")
    /// * `cache_capacity` - Maximum number of blobs to cache
    pub fn new(beacon_url: impl Into<String>, cache_capacity: u32) -> Self {
        Self {
            client: Client::new(),
            beacon_url: beacon_url.into().trim_end_matches('/').to_string(),
            cache: Arc::new(RwLock::new(LruMap::new(ByLength::new(cache_capacity)))),
        }
    }

    /// Fetch a blob by slot and versioned hash.
    ///
    /// Returns cached blob if available, otherwise fetches from Beacon API.
    pub async fn get_blob(
        &self,
        slot: u64,
        versioned_hash: B256,
    ) -> Result<Box<Blob>, PipelineError> {
        let key = (slot, versioned_hash);

        // Check cache first
        {
            let mut cache = self.cache.write().await;
            if let Some(blob) = cache.get(&key) {
                return Ok(blob.clone());
            }
        }

        // Fetch from Beacon API
        let blob = self.fetch_blob(slot, versioned_hash).await?;

        // Cache the result
        {
            let mut cache = self.cache.write().await;
            cache.insert(key, blob.clone());
        }

        Ok(blob)
    }

    /// Fetch all blobs for a slot.
    pub async fn get_blobs_for_slot(
        &self,
        slot: u64,
    ) -> Result<Vec<(B256, Box<Blob>)>, PipelineError> {
        let url = format!("{}/eth/v1/beacon/blob_sidecars/{}", self.beacon_url, slot);

        let response =
            self.client.get(&url).send().await.map_err(|e| PipelineError::Beacon(e.to_string()))?;

        if !response.status().is_success() {
            return Err(PipelineError::Beacon(format!("beacon API returned {}", response.status())));
        }

        let bundle: BeaconBlobBundle =
            response.json().await.map_err(|e| PipelineError::Beacon(e.to_string()))?;

        let mut result = Vec::with_capacity(bundle.data.len());
        for sidecar in bundle.data {
            let versioned_hash = kzg_to_versioned_hash(sidecar.kzg_commitment.as_slice());
            result.push((versioned_hash, Box::new(*sidecar.blob)));
        }

        Ok(result)
    }

    async fn fetch_blob(
        &self,
        slot: u64,
        versioned_hash: B256,
    ) -> Result<Box<Blob>, PipelineError> {
        let blobs = self.get_blobs_for_slot(slot).await?;

        for (hash, blob) in blobs {
            if hash == versioned_hash {
                return Ok(blob);
            }
        }

        Err(PipelineError::BlobNotFound(slot, format!("{:?}", versioned_hash)))
    }
}
