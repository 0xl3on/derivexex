//! Test fixtures with blob caching to survive beacon pruning (~18 days).
use alloy_eips::eip4844::Blob;
use alloy_primitives::B256;
use std::{fs, path::PathBuf};

const FIXTURES_DIR: &str = "src/tests/fixtures";

fn fixtures_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURES_DIR)
}

fn blob_cache_path(slot: u64, hash: &B256) -> PathBuf {
    fixtures_path().join(format!("blob_{}_{}.bin", slot, hash))
}

/// loads a cached blob from local file if available.
fn load_cached_blob(slot: u64, hash: &B256) -> Option<Box<Blob>> {
    let path = blob_cache_path(slot, hash);
    if !path.exists() {
        return None;
    }

    let data = fs::read(&path).ok()?;
    if data.len() != 131072 {
        return None;
    }

    Some(Box::new(Blob::from_slice(&data)))
}

pub fn save_blob_to_cache(slot: u64, hash: &B256, blob: &Blob) -> std::io::Result<()> {
    let dir = fixtures_path();
    fs::create_dir_all(&dir)?;

    let path = blob_cache_path(slot, hash);
    fs::write(&path, blob.as_ref() as &[u8])
}

/// gets a blob either from cache or by fetching from beacon API.
/// automatically caches fetched blobs for future use.
/// returns Box<Blob> to avoid 131KB stack allocation.
pub async fn get_or_fetch_blob(
    beacon: &crate::providers::BeaconBlobProvider,
    slot: u64,
    hash: B256,
) -> eyre::Result<Box<Blob>> {
    if let Some(blob) = load_cached_blob(slot, &hash) {
        tracing::debug!(slot, %hash, "loaded blob from cache");
        return Ok(blob);
    }

    tracing::info!(slot, %hash, "fetching blob from beacon API");
    let blob = beacon.get_blob_by_hash(slot, hash).await?;

    if let Err(e) = save_blob_to_cache(slot, &hash, &blob) {
        tracing::warn!(error = %e, "failed to cache blob");
    }

    Ok(Box::new(blob))
}
