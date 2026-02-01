//! Safe and finalized head tracking for L2 blocks.
//!
//! Tracks L2 block safety based on L1 inclusion:
//! - **Unsafe**: Latest L2 block derived (sequencer produced, not yet on L1)
//! - **Safe**: L2 block data has been posted to L1 (inclusion confirmed)
//! - **Finalized**: L1 block containing the data has reached PoS finality

use std::collections::VecDeque;

/// Head update event.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HeadUpdate {
    /// The most recent L2 block timestamp (may not be safe/finalized).
    pub unsafe_head: u64,
    /// L2 block timestamp whose data has been posted to L1.
    pub safe_head: u64,
    /// L2 block timestamp whose L1 inclusion has reached PoS finality.
    pub finalized_head: u64,
}

/// Entry for tracking L2 blocks and their L1 relationships.
#[derive(Debug, Clone)]
struct L2BlockEntry {
    l2_timestamp: u64,
    /// The L1 block this L2 block references (for timestamps, randomness).
    /// Kept for debugging/future use.
    #[allow(dead_code)]
    l1_origin: u64,
    /// The L1 block where the batch containing this L2 block was posted.
    l1_inclusion: u64,
}

/// Tracks L2 heads based on L1 inclusion and finality.
///
/// - `unsafe_head`: Latest derived L2 block
/// - `safe_head`: L2 block with data posted to L1 (l1_inclusion exists)
/// - `finalized_head`: L2 block whose l1_inclusion has PoS finality (64 blocks)
pub struct HeadTracker {
    /// Recent L2 blocks (oldest first).
    l2_blocks: VecDeque<L2BlockEntry>,
    /// Maximum L2 blocks to track.
    max_l2_blocks: usize,
    /// Number of L1 confirmations for finalized head (PoS finality).
    finalized_confirmations: u64,
    /// Current L1 head.
    l1_head: u64,
}

impl HeadTracker {
    /// Create a new head tracker.
    ///
    /// # Arguments
    /// * `finalized_confirmations` - L1 blocks required for finalized head (PoS finality, typically
    ///   64).
    pub fn new(finalized_confirmations: u64) -> Self {
        Self {
            l2_blocks: VecDeque::with_capacity(10000),
            max_l2_blocks: 10000,
            finalized_confirmations,
            l1_head: 0,
        }
    }

    /// Default settings for Ethereum mainnet.
    ///
    /// - Safe: immediate (data posted to L1)
    /// - Finalized: 64 L1 blocks (2 epochs, PoS finality)
    pub fn mainnet() -> Self {
        Self::new(64)
    }

    /// Track a new L2 block.
    ///
    /// # Arguments
    /// * `l2_timestamp` - The L2 block timestamp
    /// * `l1_origin` - The L1 block this L2 block references
    /// * `l1_inclusion` - The L1 block where the batch was posted
    pub fn add_l2_block(&mut self, l2_timestamp: u64, l1_origin: u64, l1_inclusion: u64) {
        if self.l2_blocks.len() >= self.max_l2_blocks {
            self.l2_blocks.pop_front();
        }
        self.l2_blocks.push_back(L2BlockEntry { l2_timestamp, l1_origin, l1_inclusion });
    }

    /// Update the L1 head and return the new head positions.
    pub fn update_l1_head(&mut self, l1_number: u64) -> HeadUpdate {
        self.l1_head = l1_number;
        self.compute_heads()
    }

    /// Get current head positions without updating L1 head.
    pub fn heads(&self) -> HeadUpdate {
        self.compute_heads()
    }

    fn compute_heads(&self) -> HeadUpdate {
        let unsafe_head = self.l2_blocks.back().map(|b| b.l2_timestamp).unwrap_or(0);

        // Safe = posted to L1 (any block we've derived from a batch is safe)
        // All tracked blocks have been derived from L1 batches, so safe = unsafe
        let safe_head = unsafe_head;

        // Finalized = L1 inclusion has PoS finality (64 confirmations)
        let finalized_l1_cutoff = self.l1_head.saturating_sub(self.finalized_confirmations);

        // Find highest L2 block whose l1_inclusion is finalized
        let finalized_head = self
            .l2_blocks
            .iter()
            .rev()
            .find(|b| b.l1_inclusion <= finalized_l1_cutoff)
            .map(|b| b.l2_timestamp)
            .unwrap_or(0);

        HeadUpdate { unsafe_head, safe_head, finalized_head }
    }

    /// Handle a reorg at the given L1 block.
    ///
    /// Removes all L2 blocks whose batch was included in L1 blocks >= `first_invalid_l1`.
    pub fn handle_reorg(&mut self, first_invalid_l1: u64) {
        self.l2_blocks.retain(|b| b.l1_inclusion < first_invalid_l1);
    }
}
