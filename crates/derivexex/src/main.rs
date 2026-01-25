mod config;
mod persistence;
mod providers;

use alloy_consensus::{BlockHeader, Transaction};
use alloy_eips::eip4844::{calc_blob_gasprice, Blob};
use alloy_primitives::{Address, Bytes, B256, U256};
use alloy_rlp::Decodable;
use config::{UnichainConfig, BASE_FEE_SCALAR, BLOB_BASE_FEE_SCALAR};
use derivexex_pipeline::{
    decode_blob_data_into, max_blob_data_size, Batch, Channel, ChannelAssembler, ChannelFrame,
    DepositedTransaction, FrameDecoder, Hardfork, L1BlockInfo, L1BlockRef, L2Block, L2BlockBuilder,
    TRANSACTION_DEPOSITED_TOPIC,
};
use futures::Future;
use futures_util::TryStreamExt;
use persistence::{DerivationCheckpoint, DerivationDb, SqliteDb};
use providers::{BeaconBlobProvider, BlobProvider};
use reth::{api::FullNodeComponents, builder::NodeTypes, primitives::EthPrimitives};
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing;
use std::{collections::HashMap, path::PathBuf};

#[derive(Debug, Default, Clone)]
struct DerivationTracker {
    pub l1_batches_processed: u64,
    pub blobs_fetched: u64,
    pub frames_decoded: u64,
    pub channels_completed: u64,
    pub l2_blocks_derived: u64,
    pub l2_txs_derived: u64,
    pub deposits_parsed: u64,
}

#[derive(Debug, Clone)]
struct BatchTransaction {
    pub tx_hash: B256,
    pub block_timestamp: u64,
    pub blob_count: usize,
    pub blob_hashes: Vec<B256>,
}

impl DerivationTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn log_stats(&self) {
        tracing::info!(
            target: "derivexex::tracker",
            l1_batches = %self.l1_batches_processed,
            blobs = %self.blobs_fetched,
            frames = %self.frames_decoded,
            channels = %self.channels_completed,
            l2_blocks = %self.l2_blocks_derived,
            l2_txs = %self.l2_txs_derived,
            deposits = %self.deposits_parsed,
            "derivation stats"
        );
    }
}

/// Wrapper around the pipeline crate's functionality with blob fetching
struct DerivationPipeline<B: BlobProvider> {
    blob_provider: B,
    assembler: ChannelAssembler,
    /// Reusable buffer for blob decoding to avoid 130KB allocation per blob
    blob_decode_buf: Vec<u8>,
    /// L2 block builder for constructing derived blocks
    block_builder: L2BlockBuilder,
    /// Pending deposits per L1 block number (epoch)
    epoch_deposits: HashMap<u64, Vec<DepositedTransaction>>,
    /// L1 block info per block number for creating L1 attributes deposits
    epoch_info: HashMap<u64, EpochInfo>,
}

/// L1 block information needed for L2 block building.
#[derive(Debug, Clone)]
struct EpochInfo {
    l1_ref: L1BlockRef,
    basefee: U256,
    /// Blob base fee calculated from excess_blob_gas via EIP-4844 formula
    blob_basefee: U256,
}

impl<B: BlobProvider> DerivationPipeline<B> {
    fn new(blob_provider: B, starting_l2_block: u64) -> Self {
        Self {
            blob_provider,
            assembler: ChannelAssembler::new(),
            // Pre-allocate the decode buffer once, reused for all blobs
            blob_decode_buf: vec![0u8; max_blob_data_size()],
            block_builder: L2BlockBuilder::new(Hardfork::Ecotone, starting_l2_block),
            epoch_deposits: HashMap::new(),
            epoch_info: HashMap::new(),
        }
    }

    /// Add a deposit for a specific L1 block (epoch).
    fn add_deposit(&mut self, l1_block_number: u64, deposit: DepositedTransaction) {
        self.epoch_deposits.entry(l1_block_number).or_default().push(deposit);
    }

    /// Register L1 block info for an epoch.
    fn register_epoch(&mut self, l1_block_number: u64, info: EpochInfo) {
        self.epoch_info.insert(l1_block_number, info);
    }

    /// Get deposits for an epoch, removing them from the pending map.
    fn take_epoch_deposits(&mut self, l1_block_number: u64) -> Vec<DepositedTransaction> {
        self.epoch_deposits.remove(&l1_block_number).unwrap_or_default()
    }

    /// Get epoch info for a given L1 block number.
    fn get_epoch_info(&self, l1_block_number: u64) -> Option<&EpochInfo> {
        self.epoch_info.get(&l1_block_number)
    }

    async fn process_blobs(
        &mut self,
        slot: u64,
        blob_hashes: &[B256],
    ) -> eyre::Result<Vec<ChannelFrame>> {
        let mut frames = Vec::new();

        for hash in blob_hashes {
            match self.blob_provider.get_blob(slot, *hash).await {
                Ok(blob) => {
                    let blob_frames = self.decode_blob(&blob)?;
                    frames.extend(blob_frames);
                }
                Err(e) => {
                    tracing::warn!(target: "derivexex::pipeline", hash = %hash, error = %e, "failed to fetch blob");
                }
            }
        }

        for frame in &frames {
            self.assembler.add_frame(frame.clone());
        }

        Ok(frames)
    }

    fn decode_blob(&mut self, blob: &Blob) -> eyre::Result<Vec<ChannelFrame>> {
        let len = decode_blob_data_into(blob, &mut self.blob_decode_buf);
        FrameDecoder::decode_frames(&self.blob_decode_buf[..len])
            .map_err(|e| eyre::eyre!("Frame decode error: {}", e))
    }

    #[inline]
    fn take_complete_channels(&mut self) -> Vec<Channel> {
        self.assembler.take_complete()
    }
}

pub async fn init<Node>(
    ctx: ExExContext<Node>,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>>
where
    Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
{
    let config = UnichainConfig::from_env().await?;

    // Initialize persistence - use env var or default path
    let db_path: PathBuf = std::env::var("DERIVEXEX_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("derivexex.db"));
    let db = SqliteDb::open(&db_path)?;

    tracing::info!(target: "derivexex::init", path = %db_path.display(), "opened persistence db");

    Ok(unichain_batch_exex(ctx, config, db))
}

pub(crate) async fn unichain_batch_exex<Node, D>(
    mut ctx: ExExContext<Node>,
    config: UnichainConfig,
    db: D,
) -> eyre::Result<()>
where
    Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
    D: DerivationDb,
{
    let expected_batcher = config.batcher;
    let batch_inbox = config.batch_inbox;

    let optimism_portal = config.optimism_portal;
    let batcher_addr = config.batcher;

    // Restore state from persistence
    let (mut tracker, starting_l2_block) = match db.load_checkpoint()? {
        Some(checkpoint) => {
            tracing::info!(
                target: "derivexex::init",
                l1_block = checkpoint.l1_block_number,
                next_l2_block = checkpoint.next_l2_block_number,
                l2_blocks = checkpoint.l2_blocks_derived,
                l2_txs = checkpoint.l2_txs_derived,
                "restored from checkpoint"
            );
            (
                DerivationTracker {
                    l2_blocks_derived: checkpoint.l2_blocks_derived,
                    l2_txs_derived: checkpoint.l2_txs_derived,
                    ..Default::default()
                },
                checkpoint.next_l2_block_number,
            )
        }
        None => {
            tracing::info!(target: "derivexex::init", "no checkpoint found, starting fresh");
            (DerivationTracker::new(), 0)
        }
    };

    // Fetch blobs from beacon API (historical blobs from finalized blocks)
    let blob_provider = BeaconBlobProvider::new(&config.beacon_url);
    let mut pipeline = DerivationPipeline::new(blob_provider, starting_l2_block);

    let mut blocks_processed: u64 = 0;
    let mut last_committed_block: Option<(u64, B256)> = None;

    while let Some(notification) = ctx.notifications.try_next().await? {
        match &notification {
            ExExNotification::ChainCommitted { new: chain } => {
                tracing::debug!(target: "derivexex::exex", chain = ?chain.range(), "chain committed");
                blocks_processed += chain.blocks().len() as u64;

                for (block, receipts) in chain.blocks_and_receipts() {
                    let block_hash = block.hash();
                    let block_number = block.number();
                    let block_timestamp = block.timestamp();
                    let block_basefee = block.base_fee_per_gas().unwrap_or(0);
                    // EIP-4844: blob_basefee = f(excess_blob_gas) from L1 block header
                    let excess_blob_gas = block.excess_blob_gas().unwrap_or(0);
                    let blob_basefee = calc_blob_gasprice(excess_blob_gas);

                    // Register this L1 block as an epoch
                    let epoch_info = EpochInfo {
                        l1_ref: L1BlockRef {
                            number: block_number,
                            hash: block_hash,
                            timestamp: block_timestamp,
                        },
                        basefee: U256::from(block_basefee),
                        blob_basefee: U256::from(blob_basefee),
                    };
                    pipeline.register_epoch(block_number, epoch_info);

                    for (log_index, log) in receipts.iter().flat_map(|r| r.logs.iter()).enumerate()
                    {
                        if log.address != optimism_portal {
                            continue;
                        }

                        if log.topics().first() != Some(&TRANSACTION_DEPOSITED_TOPIC) {
                            continue;
                        }

                        let topics: Vec<B256> = log.topics().to_vec();
                        match DepositedTransaction::from_log(
                            log.data.data.as_ref(),
                            &topics,
                            block_hash,
                            log_index as u64,
                        ) {
                            Ok(deposit) => {
                                tracker.deposits_parsed += 1;
                                tracing::debug!(
                                    target: "derivexex::deposits",
                                    from = %deposit.from,
                                    to = ?deposit.to,
                                    value = %deposit.value,
                                    source_hash = %deposit.source_hash,
                                    "parsed deposit"
                                );
                                // Add deposit to pipeline for this epoch
                                pipeline.add_deposit(block_number, deposit);
                            }
                            Err(e) => {
                                tracing::warn!(
                                    target: "derivexex::deposits",
                                    error = %e,
                                    block = %block_hash,
                                    log_index = log_index,
                                    "failed to parse deposit"
                                );
                            }
                        }
                    }
                }

                // collect batch transactions from L1 blocks, this is the data that will be used to
                // derive the L2 blocks
                let batch_txs: Vec<BatchTransaction> = chain
                    .blocks_iter()
                    .flat_map(|block| {
                        let block_timestamp = block.timestamp();

                        block.transactions_with_sender().filter_map(move |(sender, tx)| {
                            let to = tx.to()?;

                            if to != batch_inbox || *sender != expected_batcher {
                                return None;
                            }

                            let blob_hashes: Vec<B256> =
                                tx.blob_versioned_hashes().map(|h| h.to_vec()).unwrap_or_default();

                            Some(BatchTransaction {
                                tx_hash: *tx.tx_hash(),
                                block_timestamp,
                                blob_count: blob_hashes.len(),
                                blob_hashes,
                            })
                        })
                    })
                    .collect();

                // process each batch transaction through the derivation pipeline
                for batch in batch_txs {
                    if batch.blob_hashes.is_empty() {
                        continue;
                    }

                    let slot = config.timestamp_to_slot(batch.block_timestamp);

                    tracing::debug!(
                        target: "derivexex::pipeline",
                        tx = %batch.tx_hash,
                        slot = slot,
                        blobs = batch.blob_count,
                        "processing batch blobs"
                    );

                    // fetch blobs and decode frames
                    match pipeline.process_blobs(slot, &batch.blob_hashes).await {
                        Ok(frames) => {
                            tracker.blobs_fetched += batch.blob_count as u64;
                            tracker.frames_decoded += frames.len() as u64;
                        }
                        Err(e) => {
                            tracing::warn!(
                                target: "derivexex::pipeline",
                                tx = %batch.tx_hash,
                                error = %e,
                                "failed to process blobs"
                            );
                        }
                    }

                    tracker.l1_batches_processed += 1;
                }

                // check for complete channels and process them
                // processing means that we will take the complete channel, decompress to get raw
                // rlp bytes and then decode the rlp bytes to get the batch data..
                let complete_channels = pipeline.take_complete_channels();
                for channel in complete_channels {
                    match process_channel(&channel, &mut pipeline, batcher_addr, &mut tracker) {
                        Ok(blocks) => {
                            tracing::info!(
                                target: "derivexex::pipeline",
                                channel_id = %hex::encode(&channel.id[..8]),
                                l2_blocks = blocks.len(),
                                "channel processed"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                target: "derivexex::pipeline",
                                channel_id = %hex::encode(&channel.id[..8]),
                                error = %e,
                                "failed to process channel"
                            );
                        }
                    }
                }

                // Track last committed block for checkpointing
                if let Some(tip) = chain.blocks().values().last() {
                    last_committed_block = Some((tip.number, tip.hash()));
                }

                // Checkpoint every 100 blocks
                if blocks_processed % 100 == 0 {
                    tracker.log_stats();

                    if let Some((block_num, block_hash)) = last_committed_block {
                        let checkpoint = DerivationCheckpoint {
                            l1_block_number: block_num,
                            l1_block_hash: block_hash.0,
                            next_l2_block_number: pipeline.block_builder.current_block_number(),
                            l2_blocks_derived: tracker.l2_blocks_derived,
                            l2_txs_derived: tracker.l2_txs_derived,
                            timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs(),
                        };

                        if let Err(e) = db.save_checkpoint(&checkpoint) {
                            tracing::warn!(
                                target: "derivexex::persistence",
                                error = %e,
                                "failed to save checkpoint"
                            );
                        } else {
                            tracing::debug!(
                                target: "derivexex::persistence",
                                l1_block = block_num,
                                "checkpoint saved"
                            );
                        }
                    }
                }
            }
            ExExNotification::ChainReorged { old, new } => {
                tracing::warn!(
                    target: "derivexex::exex",
                    from = ?old.range(),
                    to = ?new.range(),
                    "chain reorged - clearing pending channels"
                );

                // Clear pending channels on reorg as they may be invalid
                if let Err(e) = db.clear_pending_channels() {
                    tracing::error!(target: "derivexex::persistence", error = %e, "failed to clear channels");
                }
            }
            ExExNotification::ChainReverted { old } => {
                tracing::warn!(target: "derivexex::exex", chain = ?old.range(), "chain reverted");

                // Clear pending channels on revert
                if let Err(e) = db.clear_pending_channels() {
                    tracing::error!(target: "derivexex::persistence", error = %e, "failed to clear channels");
                }
            }
        }

        if let Some(committed_chain) = notification.committed_chain() {
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
        }
    }

    tracing::info!(target: "derivexex::exex", "shutting down");
    tracker.log_stats();

    // Save final checkpoint on shutdown
    if let Some((block_num, block_hash)) = last_committed_block {
        let checkpoint = DerivationCheckpoint {
            l1_block_number: block_num,
            l1_block_hash: block_hash.0,
            next_l2_block_number: pipeline.block_builder.current_block_number(),
            l2_blocks_derived: tracker.l2_blocks_derived,
            l2_txs_derived: tracker.l2_txs_derived,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        if let Err(e) = db.save_checkpoint(&checkpoint) {
            tracing::error!(target: "derivexex::persistence", error = %e, "failed to save final checkpoint");
        } else {
            tracing::info!(target: "derivexex::persistence", l1_block = block_num, "final checkpoint saved");
        }
    }

    Ok(())
}

/// Process a complete channel: decompress, decode batches, build L2 blocks.
fn process_channel<B: BlobProvider>(
    channel: &Channel,
    pipeline: &mut DerivationPipeline<B>,
    batcher_addr: Address,
    tracker: &mut DerivationTracker,
) -> eyre::Result<Vec<L2Block>> {
    tracker.channels_completed += 1;

    let decompressed = channel.decompress()?;
    let mut cursor = decompressed.as_slice();
    let mut built_blocks = Vec::new();

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
                // Build L2 block from single batch
                if let Some(block) =
                    build_l2_block_from_single(&single, pipeline, batcher_addr, tracker)
                {
                    built_blocks.push(block);
                }
            }
            Ok(Batch::Span(span)) => {
                // Build L2 blocks from span batch
                let blocks = build_l2_blocks_from_span(&span, pipeline, batcher_addr, tracker);
                built_blocks.extend(blocks);
            }
            Err(e) => {
                tracing::warn!(
                    target: "derivexex::pipeline",
                    error = %e,
                    "failed to decode batch"
                );
            }
        }
    }

    Ok(built_blocks)
}

/// Build an L2 block from a single batch.
fn build_l2_block_from_single<B: BlobProvider>(
    single: &derivexex_pipeline::SingleBatch,
    pipeline: &mut DerivationPipeline<B>,
    batcher_addr: Address,
    tracker: &mut DerivationTracker,
) -> Option<L2Block> {
    let epoch_num = single.epoch_num;

    // Get epoch info for L1 origin
    let epoch_info = match pipeline.get_epoch_info(epoch_num) {
        Some(info) => info.clone(),
        None => {
            tracing::warn!(
                target: "derivexex::pipeline",
                epoch = epoch_num,
                "missing epoch info for single batch"
            );
            return None;
        }
    };

    // Check if this is a new epoch (need to set L1 origin and add deposits)
    let is_new_epoch = pipeline.block_builder.current_sequence_number() == 0 ||
        pipeline.get_epoch_info(epoch_num).map(|e| e.l1_ref.number) !=
            pipeline.get_epoch_info(epoch_num.saturating_sub(1)).map(|e| e.l1_ref.number);

    if is_new_epoch {
        pipeline.block_builder.set_l1_origin(epoch_info.l1_ref.clone());
        let deposits = pipeline.take_epoch_deposits(epoch_num);
        if !deposits.is_empty() {
            tracing::debug!(
                target: "derivexex::pipeline",
                epoch = epoch_num,
                deposit_count = deposits.len(),
                "adding deposits to epoch"
            );
            pipeline.block_builder.add_deposits(deposits);
        }
    }

    // Create L1BlockInfo for this block
    let sequence_number = pipeline.block_builder.current_sequence_number();
    let l1_info = L1BlockInfo::new(
        epoch_info.l1_ref.number,
        epoch_info.l1_ref.timestamp,
        epoch_info.basefee,
        epoch_info.l1_ref.hash,
        sequence_number,
        batcher_addr,
        epoch_info.blob_basefee,
        BASE_FEE_SCALAR,
        BLOB_BASE_FEE_SCALAR,
    );

    // Build the L2 block
    match pipeline.block_builder.build_block(
        single.timestamp,
        &l1_info,
        single.transactions.clone(),
    ) {
        Ok(block) => {
            tracker.l2_blocks_derived += 1;
            tracker.l2_txs_derived += block.tx_count() as u64;

            tracing::debug!(
                target: "derivexex::pipeline",
                batch_type = "single",
                l2_block = block.number,
                epoch = epoch_num,
                sequence = block.sequence_number,
                txs = block.tx_count(),
                deposits = block.deposit_count(),
                "built L2 block"
            );

            Some(block)
        }
        Err(e) => {
            tracing::warn!(
                target: "derivexex::pipeline",
                error = %e,
                epoch = epoch_num,
                "failed to build L2 block from single batch"
            );
            None
        }
    }
}

/// Build L2 blocks from a span batch.
fn build_l2_blocks_from_span<B: BlobProvider>(
    span: &derivexex_pipeline::SpanBatch,
    pipeline: &mut DerivationPipeline<B>,
    batcher_addr: Address,
    tracker: &mut DerivationTracker,
) -> Vec<L2Block> {
    let mut blocks = Vec::with_capacity(span.blocks.len());
    let base_epoch = span.l1_origin_num;

    for (i, span_element) in span.blocks.iter().enumerate() {
        // For span batches, we need to determine which epoch each block belongs to
        // based on the relative timestamp. For now, use the base epoch.
        // TODO: Properly calculate epoch based on timestamp
        let epoch_num = base_epoch;

        let epoch_info = match pipeline.get_epoch_info(epoch_num) {
            Some(info) => info.clone(),
            None => {
                tracing::warn!(
                    target: "derivexex::pipeline",
                    epoch = epoch_num,
                    block_index = i,
                    "missing epoch info for span batch element"
                );
                continue;
            }
        };

        // Set L1 origin for first block in epoch
        if i == 0 || pipeline.block_builder.current_sequence_number() == 0 {
            pipeline.block_builder.set_l1_origin(epoch_info.l1_ref.clone());
            let deposits = pipeline.take_epoch_deposits(epoch_num);
            if !deposits.is_empty() {
                pipeline.block_builder.add_deposits(deposits);
            }
        }

        let sequence_number = pipeline.block_builder.current_sequence_number();
        let l1_info = L1BlockInfo::new(
            epoch_info.l1_ref.number,
            epoch_info.l1_ref.timestamp,
            epoch_info.basefee,
            epoch_info.l1_ref.hash,
            sequence_number,
            batcher_addr,
            epoch_info.blob_basefee,
            BASE_FEE_SCALAR,
            BLOB_BASE_FEE_SCALAR,
        );

        match pipeline.block_builder.build_block(
            span_element.timestamp,
            &l1_info,
            span_element.transactions.clone(),
        ) {
            Ok(block) => {
                tracker.l2_blocks_derived += 1;
                tracker.l2_txs_derived += block.tx_count() as u64;

                tracing::debug!(
                    target: "derivexex::pipeline",
                    batch_type = "span",
                    l2_block = block.number,
                    epoch = epoch_num,
                    sequence = block.sequence_number,
                    txs = block.tx_count(),
                    "built L2 block from span"
                );

                blocks.push(block);
            }
            Err(e) => {
                tracing::warn!(
                    target: "derivexex::pipeline",
                    error = %e,
                    epoch = epoch_num,
                    block_index = i,
                    "failed to build L2 block from span batch"
                );
            }
        }
    }

    blocks
}

fn main() -> eyre::Result<()> {
    reth::cli::Cli::parse_args().run(|builder, _args| async move {
        let handle = builder
            .node(EthereumNode::default())
            .install_exex("derivexex", |ctx| async move { init(ctx).await })
            .launch()
            .await?;

        handle.wait_for_node_exit().await
    })
}

#[cfg(test)]
mod tests;
