//! Beacon API blob provider for fetching blobs from the consensus layer.

use alloy_eips::eip4844::Blob;
use alloy_primitives::B256;
use async_trait::async_trait;

use super::beacon::{BeaconBlobProvider, BeaconError};

#[derive(thiserror::Error, Debug)]
pub enum BlobProviderError {
    #[error("Beacon error: {0}")]
    Beacon(#[from] BeaconError),
}

#[async_trait]
pub trait BlobProvider: Send + Sync {
    async fn get_blob(&self, slot: u64, hash: B256) -> Result<Blob, BlobProviderError>;
}

#[async_trait]
impl BlobProvider for BeaconBlobProvider {
    async fn get_blob(&self, slot: u64, hash: B256) -> Result<Blob, BlobProviderError> {
        self.get_blob_by_hash(slot, hash).await.map_err(Into::into)
    }
}
