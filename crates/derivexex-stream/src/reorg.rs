//! L1 reorg detection.

use std::collections::VecDeque;

use alloy_primitives::B256;

/// Information about a detected reorg.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReorgEvent {
    /// The block number where the reorg was detected.
    pub detected_at: u64,
    /// The first invalid block number (fork point + 1).
    pub first_invalid: u64,
    /// Depth of the reorg (number of blocks invalidated).
    pub depth: u64,
    /// The old block hash at the fork point.
    pub old_hash: B256,
    /// The new block hash at the fork point.
    pub new_hash: B256,
}

/// Tracks recent block headers to detect reorgs.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct BlockEntry {
    number: u64,
    hash: B256,
    parent_hash: B256,
}

/// Detects L1 reorgs by tracking parent hash consistency.
pub struct ReorgDetector {
    /// Recent blocks in order (oldest first).
    recent_blocks: VecDeque<BlockEntry>,
    /// Maximum number of blocks to track.
    max_depth: usize,
}

impl ReorgDetector {
    /// Create a new reorg detector.
    ///
    /// # Arguments
    /// * `max_depth` - Maximum number of recent blocks to track (determines max reorg detection
    ///   depth).
    pub fn new(max_depth: usize) -> Self {
        Self { recent_blocks: VecDeque::with_capacity(max_depth), max_depth }
    }

    /// Process a new block and check for reorgs.
    ///
    /// Returns `Some(ReorgEvent)` if a reorg is detected.
    pub fn process_block(
        &mut self,
        number: u64,
        hash: B256,
        parent_hash: B256,
    ) -> Option<ReorgEvent> {
        // Check for parent hash mismatch
        let reorg = if let Some(expected_parent) = self.recent_blocks.back() {
            if number == expected_parent.number + 1 && parent_hash != expected_parent.hash {
                // Parent hash mismatch - reorg detected
                let (first_invalid, old_hash) = self.find_fork_point(&parent_hash);
                Some(ReorgEvent {
                    detected_at: number,
                    first_invalid,
                    depth: number.saturating_sub(first_invalid),
                    old_hash,
                    new_hash: parent_hash,
                })
            } else if number <= expected_parent.number {
                // Received an old block - this is also a reorg signal
                let first_invalid = number;
                let old_hash = self
                    .recent_blocks
                    .iter()
                    .find(|b| b.number == number)
                    .map(|b| b.hash)
                    .unwrap_or_default();
                Some(ReorgEvent {
                    detected_at: number,
                    first_invalid,
                    depth: expected_parent.number.saturating_sub(number) + 1,
                    old_hash,
                    new_hash: hash,
                })
            } else {
                None
            }
        } else {
            None
        };

        // Handle reorg: remove invalidated blocks
        if let Some(ref event) = reorg {
            self.recent_blocks.retain(|b| b.number < event.first_invalid);
        }

        // Add the new block
        if self.recent_blocks.len() >= self.max_depth {
            self.recent_blocks.pop_front();
        }
        self.recent_blocks.push_back(BlockEntry { number, hash, parent_hash });

        reorg
    }

    /// Get the latest block number being tracked.
    #[allow(dead_code)]
    pub fn latest_block(&self) -> Option<u64> {
        self.recent_blocks.back().map(|b| b.number)
    }

    /// Find the fork point by searching for a matching parent hash.
    fn find_fork_point(&self, parent_hash: &B256) -> (u64, B256) {
        // Search backwards for a block with this hash
        for block in self.recent_blocks.iter().rev() {
            if block.hash == *parent_hash {
                // Fork happened after this block
                return (block.number + 1, block.hash);
            }
        }

        // Couldn't find the fork point in our history
        // Return the oldest block we know about
        if let Some(oldest) = self.recent_blocks.front() {
            (oldest.number, oldest.hash)
        } else {
            (0, B256::ZERO)
        }
    }
}
