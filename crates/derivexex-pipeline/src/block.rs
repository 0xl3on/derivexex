//! L2 Block building from derived data.
//!
//! Combines L1 block info, deposits, and sequencer transactions into L2 blocks.
//!
//! ## L2 Block Structure
//! ```text
//! L2 Block N:
//! ├── tx[0]: L1 Attributes Deposit (system tx)
//! ├── tx[1..n]: User Deposits (from TransactionDeposited events)
//! └── tx[n+1..]: Sequencer Transactions (from batches)
//! ```
//!
//! ## Epoch and Sequence Numbers
//! - An "epoch" is a range of L2 blocks derived from one L1 block
//! - Unichain L2 has 1s block time, L1 has 12s → up to 12 L2 blocks per epoch
//! - `sequence_number` starts at 0 for first L2 block in epoch, increments for each
//! - Deposits only go in the first L2 block of an epoch (sequence_number = 0)

use alloy_primitives::{Bytes, B256};
use serde::Serialize;

use crate::{
    deposits::DepositedTransaction,
    l1_info::{Hardfork, L1BlockInfo},
};

/// Reference to an L1 block (the "origin" of derived L2 blocks).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct L1BlockRef {
    pub number: u64,
    pub hash: B256,
    pub timestamp: u64,
}

/// A transaction in an L2 block (either deposit or sequencer tx).
#[derive(Debug, Clone, Serialize)]
pub enum L2Transaction {
    /// Deposit transaction (type 0x7E) - includes L1 info and user deposits
    Deposit(DepositedTransaction),
    /// Sequencer transaction (from batch) - raw RLP-encoded tx bytes
    Sequencer(Bytes),
}

/// Builds L2 blocks from derived data.
///
/// Call `set_l1_origin` when starting a new epoch, then `build_block` for each L2 block.
#[derive(Debug, Clone)]
pub struct L2BlockBuilder {
    hardfork: Hardfork,
    /// Current L1 origin for the epoch
    l1_origin: Option<L1BlockRef>,
    /// Current L2 block number
    l2_block_number: u64,
    /// Sequence number within current epoch
    /// Note that this sequencer number has nothing to do with the
    /// sequencer numbers for frames that we dealt with when decoding blobs
    sequence_number: u64,
    /// Pending deposits for this epoch (only included in first block)
    pending_deposits: Vec<DepositedTransaction>,
}

impl L2BlockBuilder {
    /// Create a new block builder.
    ///
    /// # Arguments
    /// * `hardfork` - The hardfork to use for L1 info encoding
    /// * `starting_l2_block` - The L2 block number to start from
    pub fn new(hardfork: Hardfork, starting_l2_block: u64) -> Self {
        Self {
            hardfork,
            l1_origin: None,
            l2_block_number: starting_l2_block,
            sequence_number: 0,
            pending_deposits: Vec::new(),
        }
    }

    /// Set the L1 origin for a new epoch.
    ///
    /// Resets the sequence number to 0. Call this when moving to a new L1 block.
    pub fn set_l1_origin(&mut self, l1_origin: L1BlockRef) {
        self.l1_origin = Some(l1_origin);
        self.sequence_number = 0;
    }

    /// Add deposits for the current epoch.
    ///
    /// Deposits are only included in the first L2 block of the epoch.
    pub fn add_deposits(&mut self, deposits: impl IntoIterator<Item = DepositedTransaction>) {
        self.pending_deposits.extend(deposits);
    }

    /// Build an L2 block from batch data.
    ///
    /// # Arguments
    /// * `timestamp` - The L2 block timestamp (from the batch)
    /// * `l1_info` - L1 block info for the L1 attributes deposit
    /// * `sequencer_txs` - Sequencer transactions from the batch
    ///
    /// # Returns
    /// The built L2 block, or an error if no L1 origin is set.
    pub fn build_block(
        &mut self,
        timestamp: u64,
        l1_info: &L1BlockInfo,
        sequencer_txs: Vec<Bytes>,
    ) -> Result<L2Block, BlockBuildError> {
        let l1_origin = self.l1_origin.clone().ok_or(BlockBuildError::NoL1Origin)?;

        // Capacity: 1 (L1 info) + deposits (if epoch start) + sequencer txs
        let deposit_count =
            if self.sequence_number == 0 { 1 + self.pending_deposits.len() } else { 1 };
        let mut transactions = Vec::with_capacity(deposit_count + sequencer_txs.len());

        // 1. L1 Attributes deposit (always first)
        let l1_info_deposit = l1_info.to_deposit_tx(self.hardfork);
        transactions.push(L2Transaction::Deposit(l1_info_deposit));

        // 2. User deposits (only in first block of epoch, sequence_number = 0)
        if self.sequence_number == 0 {
            for deposit in self.pending_deposits.drain(..) {
                transactions.push(L2Transaction::Deposit(deposit));
            }
        }

        // 3. Sequencer transactions
        for tx in sequencer_txs {
            transactions.push(L2Transaction::Sequencer(tx));
        }

        let block = L2Block {
            number: self.l2_block_number,
            timestamp,
            l1_origin,
            sequence_number: self.sequence_number,
            transactions,
        };

        // Advance state
        self.l2_block_number += 1;
        self.sequence_number += 1;

        Ok(block)
    }

    /// Get the current L2 block number.
    #[inline]
    pub fn current_block_number(&self) -> u64 {
        self.l2_block_number
    }

    /// Get the current sequence number within the epoch.
    #[inline]
    pub fn current_sequence_number(&self) -> u64 {
        self.sequence_number
    }

    /// Check if there are pending deposits.
    #[inline]
    pub fn has_pending_deposits(&self) -> bool {
        !self.pending_deposits.is_empty()
    }
}

/// Errors that can occur during block building.
#[derive(Debug, Clone, thiserror::Error)]
pub enum BlockBuildError {
    #[error("no L1 origin set - call set_l1_origin first")]
    NoL1Origin,
}

/// A derived L2 block ready for execution.
#[derive(Debug, Clone, Serialize)]
#[must_use]
pub struct L2Block {
    /// L2 block number
    pub number: u64,
    /// L2 block timestamp
    pub timestamp: u64,
    /// The L1 block this was derived from
    pub l1_origin: L1BlockRef,
    /// Sequence number within the epoch (0 for first L2 block derived from this L1 block)
    pub sequence_number: u64,
    /// All transactions in order: [L1 info, deposits..., sequencer txs...]
    pub transactions: Vec<L2Transaction>,
}

impl L2Block {
    /// Returns the number of deposit transactions (including L1 info).
    pub fn deposit_count(&self) -> usize {
        self.transactions.iter().filter(|tx| matches!(tx, L2Transaction::Deposit(_))).count()
    }

    /// Returns the number of sequencer transactions.
    pub fn sequencer_tx_count(&self) -> usize {
        self.transactions.iter().filter(|tx| matches!(tx, L2Transaction::Sequencer(_))).count()
    }

    /// Returns the total transaction count.
    #[inline]
    pub fn tx_count(&self) -> usize {
        self.transactions.len()
    }

    /// Returns true if this is the first block in an epoch.
    #[inline]
    pub fn is_epoch_start(&self) -> bool {
        self.sequence_number == 0
    }
}
