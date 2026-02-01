//! Unichain-specific configuration constants.
//!
//! These values are hardcoded for Unichain mainnet (chain ID 130).

use alloy_primitives::{address, Address};

/// Unichain L2 chain ID.
pub const CHAIN_ID: u64 = 130;

/// The batcher address that submits batches to L1.
/// <https://etherscan.io/address/0x2F60A5184c63ca94f82a27100643DbAbe4F3f7Fd>
pub const BATCHER: Address = address!("2F60A5184c63ca94f82a27100643DbAbe4F3f7Fd");

/// The batch inbox address on L1 where batches are submitted.
/// <https://etherscan.io/address/0xFf00000000000000000000000000000000000130>
pub const BATCH_INBOX: Address = address!("Ff00000000000000000000000000000000000130");

/// The OptimismPortal contract on L1 for deposits.
/// <https://etherscan.io/address/0x0bd48f6B86a26D3a217d0Fa6FfE2B491B956A7a2>
pub const OPTIMISM_PORTAL: Address = address!("0bd48f6B86a26D3a217d0Fa6FfE2B491B956A7a2");

/// Unichain L2 genesis timestamp (block 0).
/// <https://mainnet.unichain.org> eth_getBlockByNumber("0x0")
pub const L2_GENESIS_TIME: u64 = 1730748359; // 2024-11-04 19:25:59 UTC

/// Unichain L2 block time in seconds.
pub const L2_BLOCK_TIME: u64 = 1;

/// Fee scalars from SystemConfig.scalar() on 2026-01-24.
pub const BASE_FEE_SCALAR: u32 = 2000;
pub const BLOB_BASE_FEE_SCALAR: u32 = 900_000;

/// Default L1 WebSocket RPC URL.
pub const DEFAULT_L1_WS_URL: &str = "wss://eth.llamarpc.com";

/// Default Beacon API URL.
pub const DEFAULT_BEACON_URL: &str = "https://ethereum-beacon-api.publicnode.com";
