//! High-level derivation: Channel → L2 Blocks.
//!
//! Provides a reusable `Deriver` that processes channels into L2 blocks,
//! used by both the ExEx and tests.

use alloy_primitives::{Address, Bytes, U256};
use alloy_rlp::Decodable;
use bon::Builder;
use std::collections::HashMap;

use crate::{
    Batch, Channel, DepositedTransaction, Hardfork, L1BlockInfo, L1BlockRef, L2Block,
    L2BlockBuilder,
};

/// Configuration for the deriver (defaults are Unichain values).
#[derive(Debug, Clone, Builder)]
pub struct DeriverConfig {
    #[builder(default)]
    pub hardfork: Hardfork,
    pub batcher_addr: Address,
    #[builder(default = 2000)]
    pub base_fee_scalar: u32,
    #[builder(default = 900_000)]
    pub blob_base_fee_scalar: u32,
    pub l2_genesis_time: u64,
    #[builder(default = 1)]
    pub l2_block_time: u64,
}

/// L1 epoch information needed to derive L2 blocks.
#[derive(Debug, Clone)]
pub struct EpochInfo {
    pub l1_ref: L1BlockRef,
    pub basefee: U256,
    pub blob_basefee: U256,
}

/// High-level deriver that processes channels into L2 blocks.
#[derive(Clone)]
pub struct Deriver {
    config: DeriverConfig,
    block_builder: L2BlockBuilder,
    /// Pending deposits per L1 block number (epoch)
    epoch_deposits: HashMap<u64, Vec<DepositedTransaction>>,
    /// L1 block info per block number
    epoch_info: HashMap<u64, EpochInfo>,
}

/// Stats from processing a channel.
#[derive(Debug, Default, Clone)]
#[must_use]
pub struct ChannelResult {
    pub blocks_built: u64,
    pub txs_count: u64,
    pub blocks: Vec<L2Block>,
}

impl Deriver {
    /// Create a new deriver.
    pub fn new(config: DeriverConfig, starting_l2_block: u64) -> Self {
        Self {
            block_builder: L2BlockBuilder::new(config.hardfork, starting_l2_block),
            config,
            epoch_deposits: HashMap::new(),
            epoch_info: HashMap::new(),
        }
    }

    /// Register L1 block info for an epoch.
    #[inline]
    pub fn register_epoch(&mut self, l1_block_number: u64, info: EpochInfo) {
        self.epoch_info.insert(l1_block_number, info);
    }

    /// Add a deposit for a specific L1 block (epoch).
    #[inline]
    pub fn add_deposit(&mut self, l1_block_number: u64, deposit: DepositedTransaction) {
        self.epoch_deposits.entry(l1_block_number).or_default().push(deposit);
    }

    /// Add multiple deposits for a specific L1 block (epoch).
    #[inline]
    pub fn add_deposits(
        &mut self,
        l1_block_number: u64,
        deposits: impl IntoIterator<Item = DepositedTransaction>,
    ) {
        self.epoch_deposits.entry(l1_block_number).or_default().extend(deposits);
    }

    /// Get the current L2 block number.
    #[inline]
    pub fn current_block_number(&self) -> u64 {
        self.block_builder.current_block_number()
    }

    /// Clear all epoch data (info and deposits) for L1 blocks >= first_invalid_block.
    /// Call this on L1 reorg to remove stale state.
    ///
    /// Note: This doesn't roll back the L2 block number. For a production system,
    /// you'd need to track which L2 blocks came from which L1 blocks and revert accordingly.
    /// For this learning project, we accept that derived L2 block numbers may drift on reorg.
    pub fn clear_epochs_from(&mut self, first_invalid_block: u64) {
        self.epoch_info.retain(|&block_num, _| block_num < first_invalid_block);
        self.epoch_deposits.retain(|&block_num, _| block_num < first_invalid_block);
    }

    /// Process a complete channel into L2 blocks.
    pub fn process_channel(&mut self, channel: &Channel) -> Result<ChannelResult, DeriveError> {
        let decompressed = channel.decompress().map_err(DeriveError::Channel)?;
        let mut cursor = decompressed.as_slice();
        let mut result = ChannelResult::default();

        while !cursor.is_empty() {
            let batch_bytes = match Bytes::decode(&mut cursor) {
                Ok(b) => b,
                Err(_) => break,
            };

            if batch_bytes.is_empty() {
                continue;
            }

            match Batch::decode(&batch_bytes) {
                Ok(Batch::Single(single)) => {
                    if let Some(block) = self.build_block_from_single(&single)? {
                        result.txs_count += block.tx_count() as u64;
                        result.blocks_built += 1;
                        result.blocks.push(block);
                    }
                }
                Ok(Batch::Span(span)) => {
                    let blocks = self.build_blocks_from_span(&span)?;
                    for block in blocks {
                        result.txs_count += block.tx_count() as u64;
                        result.blocks_built += 1;
                        result.blocks.push(block);
                    }
                }
                Err(e) => {
                    return Err(DeriveError::Batch(e));
                }
            }
        }

        Ok(result)
    }

    /// Build an L2 block from a single batch.
    fn build_block_from_single(
        &mut self,
        single: &crate::SingleBatch,
    ) -> Result<Option<L2Block>, DeriveError> {
        let epoch_num = single.epoch_num;

        let epoch_info = match self.epoch_info.get(&epoch_num) {
            Some(info) => info.clone(),
            None => return Err(DeriveError::MissingEpoch(epoch_num)),
        };

        // Check if this is a new epoch
        let current_seq = self.block_builder.current_sequence_number();
        if current_seq == 0 {
            self.block_builder.set_l1_origin(epoch_info.l1_ref.clone());
            if let Some(deposits) = self.epoch_deposits.remove(&epoch_num) {
                self.block_builder.add_deposits(deposits);
            }
        }

        let l1_info = self.create_l1_info(&epoch_info);

        let block = self
            .block_builder
            .build_block(single.timestamp, &l1_info, single.transactions.clone())
            .map_err(DeriveError::BlockBuild)?;

        Ok(Some(block))
    }

    /// Build L2 blocks from a span batch.
    fn build_blocks_from_span(
        &mut self,
        span: &crate::SpanBatch,
    ) -> Result<Vec<L2Block>, DeriveError> {
        let mut blocks = Vec::with_capacity(span.blocks.len());

        // Get first block's epoch (already computed correctly in batch decoding)
        let first_epoch = span.blocks.first().map(|b| b.epoch_num).unwrap_or(span.l1_origin_num);

        // Track current epoch info
        let mut current_epoch_info = match self.epoch_info.get(&first_epoch) {
            Some(info) => info.clone(),
            None => return Err(DeriveError::MissingEpoch(first_epoch)),
        };
        let mut current_epoch_num = first_epoch;

        for (i, element) in span.blocks.iter().enumerate() {
            // Compute L2 block timestamp: genesis + rel_timestamp + block_idx * block_time
            let timestamp = self.config.l2_genesis_time +
                span.rel_timestamp +
                (i as u64) * self.config.l2_block_time;

            // Check if epoch changed (element.epoch_num differs from current)
            if element.epoch_num != current_epoch_num {
                // Try to get new epoch info, otherwise keep using current
                if let Some(info) = self.epoch_info.get(&element.epoch_num) {
                    current_epoch_info = info.clone();
                    current_epoch_num = element.epoch_num;

                    // New epoch - set L1 origin and add deposits
                    self.block_builder.set_l1_origin(current_epoch_info.l1_ref.clone());
                    if let Some(deposits) = self.epoch_deposits.remove(&current_epoch_num) {
                        self.block_builder.add_deposits(deposits);
                    }
                }
                // If epoch info not found, continue using previous epoch info
                // This matches real derivation behavior where we may not have all L1 blocks yet
            }

            // Set L1 origin for first block
            if i == 0 {
                self.block_builder.set_l1_origin(current_epoch_info.l1_ref.clone());
                if let Some(deposits) = self.epoch_deposits.remove(&current_epoch_num) {
                    self.block_builder.add_deposits(deposits);
                }
            }

            let l1_info = self.create_l1_info(&current_epoch_info);

            match self.block_builder.build_block(timestamp, &l1_info, element.transactions.clone())
            {
                Ok(block) => blocks.push(block),
                Err(e) => return Err(DeriveError::BlockBuild(e)),
            }
        }

        Ok(blocks)
    }

    /// Create L1BlockInfo from epoch info.
    fn create_l1_info(&self, epoch_info: &EpochInfo) -> L1BlockInfo {
        L1BlockInfo::new(
            epoch_info.l1_ref.number,
            epoch_info.l1_ref.timestamp,
            epoch_info.basefee,
            epoch_info.l1_ref.hash,
            self.block_builder.current_sequence_number(),
            self.config.batcher_addr,
            epoch_info.blob_basefee,
            self.config.base_fee_scalar,
            self.config.blob_base_fee_scalar,
        )
    }
}

/// Errors that can occur during derivation.
#[derive(Debug, thiserror::Error)]
pub enum DeriveError {
    #[error("channel error: {0}")]
    Channel(#[from] crate::ChannelError),
    #[error("batch decode error: {0}")]
    Batch(#[from] crate::BatchError),
    #[error("block build error: {0}")]
    BlockBuild(#[from] crate::BlockBuildError),
    #[error("missing epoch info for L1 block {0}")]
    MissingEpoch(u64),
}
