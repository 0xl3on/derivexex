//! Shared types for the derivexex crate family.

use serde::{Deserialize, Serialize};

/// Serialized state of a pending (incomplete) channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelState {
    /// 16-byte channel ID.
    pub id: [u8; 16],
    /// Serialized frames received so far.
    pub frames: Vec<FrameState>,
    /// Whether we've seen the last frame.
    pub is_closed: bool,
    /// Highest frame number seen.
    pub highest_frame: u16,
    /// L1 block when first frame was seen (for expiry).
    pub opened_at_l1_block: u64,
}

/// Serialized state of a single frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameState {
    pub frame_number: u16,
    pub data: Vec<u8>,
    pub is_last: bool,
}

/// Checkpoint of derivation progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivationCheckpoint {
    /// Last fully processed L1 block number.
    pub l1_block_number: u64,
    /// Last fully processed L1 block hash.
    pub l1_block_hash: [u8; 32],
    /// Next L2 block number to derive.
    /// This is the actual block number, not a count.
    pub next_l2_block_number: u64,
    /// Number of L2 blocks derived so far (for stats).
    pub l2_blocks_derived: u64,
    /// Number of L2 transactions derived so far (for stats).
    pub l2_txs_derived: u64,
    /// Timestamp of this checkpoint.
    pub timestamp: u64,
}

impl DerivationCheckpoint {
    pub fn new(l1_block_number: u64, l1_block_hash: [u8; 32], next_l2_block_number: u64) -> Self {
        Self {
            l1_block_number,
            l1_block_hash,
            next_l2_block_number,
            l2_blocks_derived: 0,
            l2_txs_derived: 0,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }
}
