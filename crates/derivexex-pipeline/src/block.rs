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

use crate::{
    deposits::DepositedTransaction,
    l1_info::{Hardfork, L1BlockInfo},
};

/// Reference to an L1 block (the "origin" of derived L2 blocks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L1BlockRef {
    pub number: u64,
    pub hash: B256,
    pub timestamp: u64,
}

/// A transaction in an L2 block (either deposit or sequencer tx).
#[derive(Debug, Clone)]
pub enum L2Transaction {
    /// Deposit transaction (type 0x7E) - includes L1 info and user deposits
    Deposit(DepositedTransaction),
    /// Sequencer transaction (from batch) - raw RLP-encoded tx bytes
    Sequencer(Bytes),
}

/// Builds L2 blocks from derived data.
///
/// Call `set_l1_origin` when starting a new epoch, then `build_block` for each L2 block.
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
#[derive(Debug, Clone)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, U256};

    fn mock_l1_origin() -> L1BlockRef {
        L1BlockRef { number: 21000000, hash: B256::repeat_byte(0x11), timestamp: 1700000000 }
    }

    fn mock_l1_info(sequence_number: u64) -> L1BlockInfo {
        L1BlockInfo::new(
            21000000,
            1700000000,
            U256::from(30_000_000_000u64), // 30 gwei
            B256::repeat_byte(0x11),
            sequence_number,
            address!("2F60A5184c63ca94f82a27100643DbAbe4F3f7Fd"),
            U256::from(1u64),
            1000,
            500,
        )
    }

    fn mock_deposit() -> DepositedTransaction {
        DepositedTransaction {
            source_hash: B256::repeat_byte(0xab),
            from: address!("1111111111111111111111111111111111111111"),
            to: Some(address!("2222222222222222222222222222222222222222")),
            mint: U256::from(1_000_000_000_000_000_000u64), // 1 ETH
            value: U256::from(1_000_000_000_000_000_000u64),
            gas_limit: 100_000,
            is_system_tx: false,
            data: Bytes::new(),
        }
    }

    #[test]
    fn test_build_first_block_with_deposits() {
        let mut builder = L2BlockBuilder::new(Hardfork::Ecotone, 1000);
        builder.set_l1_origin(mock_l1_origin());
        builder.add_deposits([mock_deposit(), mock_deposit()]);

        let l1_info = mock_l1_info(0);
        let sequencer_txs = vec![Bytes::from_static(&[0x01, 0x02, 0x03])];

        let block = builder.build_block(1700000002, &l1_info, sequencer_txs).unwrap();

        assert_eq!(block.number, 1000);
        assert_eq!(block.timestamp, 1700000002);
        assert_eq!(block.sequence_number, 0);
        assert!(block.is_epoch_start());

        // L1 info + 2 deposits + 1 sequencer tx
        assert_eq!(block.tx_count(), 4);
        assert_eq!(block.deposit_count(), 3); // L1 info + 2 user deposits
        assert_eq!(block.sequencer_tx_count(), 1);
    }

    #[test]
    fn test_subsequent_blocks_no_deposits() {
        let mut builder = L2BlockBuilder::new(Hardfork::Ecotone, 1000);
        builder.set_l1_origin(mock_l1_origin());
        builder.add_deposits([mock_deposit()]);

        // First block gets the deposit
        let block1 = builder.build_block(1700000002, &mock_l1_info(0), vec![]).unwrap();
        assert_eq!(block1.deposit_count(), 2); // L1 info + 1 deposit

        // Second block in same epoch - no deposits
        let block2 = builder.build_block(1700000004, &mock_l1_info(1), vec![]).unwrap();
        assert_eq!(block2.deposit_count(), 1); // Only L1 info
        assert_eq!(block2.sequence_number, 1);
        assert!(!block2.is_epoch_start());
    }

    #[test]
    fn test_new_epoch_resets_sequence_number() {
        let mut builder = L2BlockBuilder::new(Hardfork::Ecotone, 1000);

        // First epoch
        builder.set_l1_origin(mock_l1_origin());
        let _ = builder.build_block(1700000002, &mock_l1_info(0), vec![]).unwrap();
        let _ = builder.build_block(1700000004, &mock_l1_info(1), vec![]).unwrap();
        assert_eq!(builder.current_sequence_number(), 2);

        // New epoch
        builder.set_l1_origin(L1BlockRef {
            number: 21000001,
            hash: B256::repeat_byte(0x22),
            timestamp: 1700000012,
        });
        assert_eq!(builder.current_sequence_number(), 0);

        let block = builder.build_block(1700000014, &mock_l1_info(0), vec![]).unwrap();
        assert_eq!(block.sequence_number, 0);
        assert!(block.is_epoch_start());
    }

    #[test]
    fn test_error_without_l1_origin() {
        let mut builder = L2BlockBuilder::new(Hardfork::Ecotone, 1000);

        let result = builder.build_block(1700000002, &mock_l1_info(0), vec![]);
        assert!(matches!(result, Err(BlockBuildError::NoL1Origin)));
    }

    #[test]
    fn test_block_number_advances() {
        let mut builder = L2BlockBuilder::new(Hardfork::Ecotone, 5000);
        builder.set_l1_origin(mock_l1_origin());

        assert_eq!(builder.current_block_number(), 5000);

        let block1 = builder.build_block(1, &mock_l1_info(0), vec![]).unwrap();
        assert_eq!(block1.number, 5000);
        assert_eq!(builder.current_block_number(), 5001);

        let block2 = builder.build_block(2, &mock_l1_info(1), vec![]).unwrap();
        assert_eq!(block2.number, 5001);
        assert_eq!(builder.current_block_number(), 5002);
    }

    #[test]
    fn test_l1_origin_in_block() {
        let mut builder = L2BlockBuilder::new(Hardfork::Ecotone, 1000);
        let origin = mock_l1_origin();
        builder.set_l1_origin(origin.clone());

        let block = builder.build_block(1, &mock_l1_info(0), vec![]).unwrap();

        assert_eq!(block.l1_origin.number, origin.number);
        assert_eq!(block.l1_origin.hash, origin.hash);
        assert_eq!(block.l1_origin.timestamp, origin.timestamp);
    }
}
