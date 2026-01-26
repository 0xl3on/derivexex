use alloy_primitives::{address, Address};
use std::env;

use crate::providers::{BeaconBlobProvider, BeaconError};

pub const BATCH_INBOX: Address = address!("Ff00000000000000000000000000000000000130");
pub const BATCHER: Address = address!("2F60A5184c63ca94f82a27100643DbAbe4F3f7Fd");
pub const OPTIMISM_PORTAL: Address = address!("0bd48f6B86a26D3a217d0Fa6FfE2B491B956A7a2");

/// Unichain SystemConfig contract on L1 (Ethereum mainnet).
/// https://etherscan.io/address/0xc407398d063f942febbcc6f80a156b47f3f1bda6
#[allow(dead_code)]
pub const SYSTEM_CONFIG: Address = address!("c407398d063f942febbcc6f80a156b47f3f1bda6");

/// Fee scalars from SystemConfig.scalar() on 2026-01-24.
/// - baseFeeScalar at bytes[28:32] = 2000
/// - blobBaseFeeScalar at bytes[24:28] = 900000
pub const BASE_FEE_SCALAR: u32 = 2000;
pub const BLOB_BASE_FEE_SCALAR: u32 = 900_000;

/// Unichain L2 genesis timestamp (block 0).
/// Fetched from: https://mainnet.unichain.org eth_getBlockByNumber("0x0")
pub const L2_GENESIS_TIME: u64 = 1730748359; // 2024-11-04 19:25:59 UTC

/// Unichain L2 block time in seconds.
pub const L2_BLOCK_TIME: u64 = 1;

#[derive(Debug, Clone)]
pub struct UnichainConfig {
    pub batch_inbox: Address,
    pub batcher: Address,
    pub optimism_portal: Address,
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
            optimism_portal: OPTIMISM_PORTAL,
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
