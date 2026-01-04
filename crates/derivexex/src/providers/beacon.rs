//! Basic and clean beacon api wrapper for fetching blob and chain data

use alloy_eips::eip4844::{kzg_to_versioned_hash, Blob};
use alloy_primitives::B256;
use alloy_rpc_types_beacon::sidecar::BeaconBlobBundle;
use reqwest::Client;

use super::beacon_api_types::{GenesisResponse, SpecResponse};

const BLOB_SIDECARS_PATH: &str = "/eth/v1/beacon/blob_sidecars";
const SPEC_PATH: &str = "/eth/v1/config/spec";
const GENESIS_PATH: &str = "/eth/v1/beacon/genesis";

#[derive(thiserror::Error, Debug)]
pub enum BeaconError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Blob not found for hash {0}")]
    BlobNotFound(B256),
}

pub struct BeaconBlobProvider {
    client: Client,
    base: String,
}

impl BeaconBlobProvider {
    pub fn new(beacon_url: &str) -> Self {
        Self { client: Client::new(), base: beacon_url.trim_end_matches('/').to_string() }
    }

    #[inline]
    fn blob_sidecars_url(&self, slot: u64) -> String {
        format!("{}{}/{}", self.base, BLOB_SIDECARS_PATH, slot)
    }

    pub async fn genesis(&self) -> Result<GenesisResponse, BeaconError> {
        let url = format!("{}{}", self.base, GENESIS_PATH);

        Ok(self.client.get(&url).send().await?.json().await?)
    }

    pub async fn spec(&self) -> Result<SpecResponse, BeaconError> {
        let url = format!("{}{}", self.base, SPEC_PATH);

        Ok(self.client.get(&url).send().await?.json().await?)
    }

    pub async fn get_blobs_by_slot(&self, slot: u64) -> Result<BeaconBlobBundle, BeaconError> {
        let url = self.blob_sidecars_url(slot);

        let response: BeaconBlobBundle = self.client.get(&url).send().await?.json().await?;

        Ok(response)
    }

    pub async fn get_blob_by_hash(
        &self,
        slot: u64,
        versioned_hash: B256,
    ) -> Result<Blob, BeaconError> {
        let bundle = self.get_blobs_by_slot(slot).await?;

        for sidecar in bundle.data {
            // each blob has a kzg commitment, we can use it to get the hash.
            // a kzg commitment is a way to verify the integrity of the blob, explaining in a very
            // simple way!
            let hash = kzg_to_versioned_hash(sidecar.kzg_commitment.as_slice());

            if hash == versioned_hash {
                return Ok(*sidecar.blob);
            }
        }

        Err(BeaconError::BlobNotFound(versioned_hash))
    }
}
