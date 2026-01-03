//! Trait for abstracing blob fetching, where we first try to do so on Reth DB then on beacon api
//! fallback.
//! Inspired by Georgios suggestion here: https://x.com/gakonst/status/1814668936887042303/photo/1

use alloy_eips::eip4844::Blob;
use alloy_primitives::B256;
use async_trait::async_trait;
use reth_transaction_pool::{BlobStore, BlobStoreError, TransactionPool};
use std::sync::Arc;

use super::beacon::{BeaconBlobProvider, BeaconError};

#[derive(thiserror::Error, Debug)]
pub enum BlobProviderError {
    #[error("Pool error: {0}")]
    Pool(#[from] BlobStoreError),
    #[error("Beacon error: {0}")]
    Beacon(#[from] BeaconError),
}

#[async_trait]
pub trait BlobProvider: Send + Sync {
    async fn get_blob(&self, slot: u64, hash: B256) -> Result<Blob, BlobProviderError>;
}

/// combined provider: tries Reth DB pool first, falls back to beacon API.
pub struct PoolBeaconBlobProvider<P: TransactionPool> {
    pool: Arc<P>,
    beacon: BeaconBlobProvider,
}

impl<P: TransactionPool> PoolBeaconBlobProvider<P> {
    pub fn new(pool: Arc<P>, beacon_url: &str) -> Self {
        Self { pool: pool.clone(), beacon: BeaconBlobProvider::new(beacon_url) }
    }
}

#[async_trait]
impl<P> BlobProvider for PoolBeaconBlobProvider<P>
where
    P: TransactionPool + Send + Sync + 'static,
{
    async fn get_blob(&self, slot: u64, hash: B256) -> Result<Blob, BlobProviderError> {
        // try pool first (fast, local)
        match self.pool.get_blobs_for_versioned_hashes_v1(&[hash]) {
            Ok(blobs) => {
                if let Some(Some(blob_and_proof)) = blobs.first() {
                    tracing::trace!(target: "derivexex::blob", %hash, "blob from pool");
                    return Ok(*blob_and_proof.blob.clone());
                }
            }
            Err(e) => {
                tracing::trace!(target: "derivexex::blob", %hash, error = %e, "pool lookup failed");
            }
        }

        // fallback to beacon API
        tracing::trace!(target: "derivexex::blob", %hash, slot, "fetching from beacon");
        let blob = self.beacon.get_blob_by_hash(slot, hash).await?;
        Ok(blob)
    }
}

/// beacon-only provider for when pool is not available, like on tests or during sync.
#[async_trait]
impl BlobProvider for BeaconBlobProvider {
    async fn get_blob(&self, slot: u64, hash: B256) -> Result<Blob, BlobProviderError> {
        self.get_blob_by_hash(slot, hash).await.map_err(Into::into)
    }
}
