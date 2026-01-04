mod config;
mod persistence;
mod providers;

use alloy_consensus::{BlockHeader, Transaction};
use alloy_eips::eip4844::Blob;
use alloy_primitives::{Bytes, B256};
use alloy_rlp::Decodable;
use config::UnichainConfig;
use derivexex_pipeline::{
    decode_blob_data_into, max_blob_data_size, Batch, Channel, ChannelAssembler, ChannelFrame,
    FrameDecoder,
};
use futures::Future;
use futures_util::TryStreamExt;
use persistence::{DerivationCheckpoint, DerivationDb, SqliteDb};
use providers::{BeaconBlobProvider, BlobProvider};
use reth::{api::FullNodeComponents, builder::NodeTypes, primitives::EthPrimitives};
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing;
use std::path::PathBuf;

#[derive(Debug, Default, Clone)]
struct UnichainBatchTracker {
    pub l1_batches_processed: u64,
    pub blobs_fetched: u64,
    pub frames_decoded: u64,
    pub channels_completed: u64,
    pub l2_blocks_derived: u64,
    pub l2_txs_derived: u64,
}

#[derive(Debug, Clone)]
struct BatchTransaction {
    pub tx_hash: B256,
    pub block_timestamp: u64,
    pub blob_count: usize,
    pub blob_hashes: Vec<B256>,
}

impl UnichainBatchTracker {
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
}

impl<B: BlobProvider> DerivationPipeline<B> {
    fn new(blob_provider: B) -> Self {
        Self {
            blob_provider,
            assembler: ChannelAssembler::new(),
            // Pre-allocate the decode buffer once, reused for all blobs
            blob_decode_buf: vec![0u8; max_blob_data_size()],
        }
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

    // Fetch blobs from beacon API (historical blobs from finalized blocks)
    let blob_provider = BeaconBlobProvider::new(&config.beacon_url);
    let mut pipeline = DerivationPipeline::new(blob_provider);

    // Restore state from persistence
    let mut tracker = match db.load_checkpoint()? {
        Some(checkpoint) => {
            tracing::info!(
                target: "derivexex::init",
                l1_block = checkpoint.l1_block_number,
                l2_blocks = checkpoint.l2_blocks_derived,
                l2_txs = checkpoint.l2_txs_derived,
                "restored from checkpoint"
            );
            UnichainBatchTracker {
                l2_blocks_derived: checkpoint.l2_blocks_derived,
                l2_txs_derived: checkpoint.l2_txs_derived,
                ..Default::default()
            }
        }
        None => {
            tracing::info!(target: "derivexex::init", "no checkpoint found, starting fresh");
            UnichainBatchTracker::new()
        }
    };

    let mut blocks_processed: u64 = 0;
    let mut last_committed_block: Option<(u64, B256)> = None;

    while let Some(notification) = ctx.notifications.try_next().await? {
        match &notification {
            ExExNotification::ChainCommitted { new: chain } => {
                tracing::debug!(target: "derivexex::exex", chain = ?chain.range(), "chain committed");
                blocks_processed += chain.blocks().len() as u64;

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
                    match process_channel(&channel, &mut tracker) {
                        Ok(()) => {
                            tracing::info!(
                                target: "derivexex::pipeline",
                                channel_id = %hex::encode(&channel.id[..8]),
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

/// process a complete channel: decompress, decode batches, extract L2 data
fn process_channel(channel: &Channel, tracker: &mut UnichainBatchTracker) -> eyre::Result<()> {
    tracker.channels_completed += 1;

    let decompressed = channel.decompress()?;

    // rlp decode to get raw batch bytes
    let mut cursor = decompressed.as_slice();

    while !cursor.is_empty() {
        let batch_bytes = match Bytes::decode(&mut cursor) {
            Ok(b) => b,
            Err(_) => break,
        };

        if batch_bytes.is_empty() {
            continue;
        }

        // decode the raw rlp encoded batch bytes to get the batch data
        match Batch::decode(&batch_bytes) {
            Ok(Batch::Single(single)) => {
                tracker.l2_blocks_derived += 1;
                tracker.l2_txs_derived += single.transactions.len() as u64;

                tracing::debug!(
                    target: "derivexex::pipeline",
                    batch_type = "single",
                    epoch = single.epoch_num,
                    timestamp = single.timestamp,
                    txs = single.transactions.len(),
                    "decoded batch"
                );
            }
            Ok(Batch::Span(span)) => {
                let block_count = span.blocks.len() as u64;
                let tx_count: u64 = span.blocks.iter().map(|b| b.transactions.len() as u64).sum();

                tracker.l2_blocks_derived += block_count;
                tracker.l2_txs_derived += tx_count;

                tracing::debug!(
                    target: "derivexex::pipeline",
                    batch_type = "span",
                    l1_origin = span.l1_origin_num,
                    l2_blocks = block_count,
                    l2_txs = tx_count,
                    "decoded batch"
                );
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

    Ok(())
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
