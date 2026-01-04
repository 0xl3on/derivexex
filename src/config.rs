use alloy_primitives::{address, Address};
use std::env;

use crate::providers::{BeaconBlobProvider, BeaconError};

pub const BATCH_INBOX: Address = address!("Ff00000000000000000000000000000000000130");
pub const BATCHER: Address = address!("2F60A5184c63ca94f82a27100643DbAbe4F3f7Fd");

#[derive(Debug, Clone)]
pub struct UnichainConfig {
    pub batch_inbox: Address,
    pub batcher: Address,
    pub beacon_url: String,
    pub beacon_genesis_time: u64,
    pub l1_seconds_per_slot: u64,
}

impl UnichainConfig {
    pub async fn from_env() -> eyre::Result<Self> {
        let beacon_url = env::var("BEACON_API")
            .map_err(|_| eyre::eyre!("BEACON_API environment variable is required"))?;

        let beacon = BeaconBlobProvider::new(&beacon_url);

        // send both requests in parallel, revert if one fails
        let (genesis, spec) = tokio::try_join!(beacon.genesis(), beacon.spec())
            .map_err(|e: BeaconError| eyre::eyre!("Failed to fetch beacon config: {}", e))?;

        Ok(Self {
            batch_inbox: BATCH_INBOX,
            batcher: BATCHER,
            beacon_url,
            beacon_genesis_time: genesis.data.genesis_time,
            l1_seconds_per_slot: spec.data.seconds_per_slot,
        })
    }

    #[inline]
    pub fn timestamp_to_slot(&self, timestamp: u64) -> u64 {
        (timestamp.saturating_sub(self.beacon_genesis_time)) / self.l1_seconds_per_slot
    }
}
