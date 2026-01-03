mod config;
mod pipeline;
mod providers;

use alloy_consensus::{BlockHeader, Typed2718};
use alloy_primitives::{Address, B256};
use config::UnichainConfig;
use futures::Future;
use futures_util::TryStreamExt;
use pipeline::DerivationPipeline;
use reth::{
    api::FullNodeComponents, builder::NodeTypes, primitives::EthPrimitives,
    rpc::types::TransactionTrait,
};
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing;

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

pub async fn init<Node>(
    ctx: ExExContext<Node>,
) -> eyre::Result<impl Future<Output = eyre::Result<()>>>
where
    Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
{
    let config = UnichainConfig::from_env().await?;
    Ok(unichain_batch_exex(ctx, config))
}

pub(crate) async fn unichain_batch_exex<Node>(
    mut ctx: ExExContext<Node>,
    config: UnichainConfig,
) -> eyre::Result<()>
where
    Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
{
    let expected_batcher = config.batcher;
    let batch_inbox = config.batch_inbox;
    let mut pipeline = DerivationPipeline::new(config.clone());
    let mut tracker = UnichainBatchTracker::new();
    let mut blocks_processed: u64 = 0;

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

                if blocks_processed % 100 == 0 {
                    tracker.log_stats();
                }
            }
            ExExNotification::ChainReorged { old, new } => {
                tracing::debug!(
                    target: "derivexex::exex",
                    from = ?old.range(),
                    to = ?new.range(),
                    "chain reorged"
                );
                // TODO: handle this
            }
            ExExNotification::ChainReverted { old } => {
                tracing::debug!(target: "derivexex::exex", chain = ?old.range(), "chain reverted");
                // TODO: handle this
            }
        }

        if let Some(committed_chain) = notification.committed_chain() {
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
        }
    }

    tracing::info!(target: "derivexex::exex", "shutting down");
    tracker.log_stats();

    Ok(())
}

/// process a complete channel: decompress, decode batches, extract L2 data
fn process_channel(
    channel: &pipeline::Channel,
    tracker: &mut UnichainBatchTracker,
) -> eyre::Result<()> {
    use alloy_primitives::Bytes;
    use alloy_rlp::Decodable;

    tracker.channels_completed += 1;

    let decompressed = channel.decompress()?;

    // rlp decode to get raw batch bytes
    let mut cursor = decompressed.as_slice();

    while !cursor.is_empty() {
        // decode the rlp bytes to get the batch data..
        let batch_bytes = match Bytes::decode(&mut cursor) {
            Ok(b) => b,
            Err(_) => break,
        };

        if batch_bytes.is_empty() {
            continue;
        }

        let batch_type = batch_bytes[0];

        // decode span batch (type=1)
        // span batches are multiple blocks that are compressed together
        if batch_type == 1 {
            if let Some((block_count, tx_count)) = decode_span_batch_summary(&batch_bytes[1..]) {
                tracker.l2_blocks_derived += block_count;
                tracker.l2_txs_derived += tx_count;

                tracing::debug!(
                    target: "derivexex::pipeline",
                    batch_type = "span",
                    l2_blocks = block_count,
                    l2_txs = tx_count,
                    "decoded batch"
                );
            }
        } else if batch_type == 0 {
            // single batch (type=0) - one block
            // after delta upgrade,
            todo!("single batch (type=0) - one block");
        }
    }

    Ok(())
}

/// extracts block_count and total_tx_count from a span batch. this is a temporary function but we
/// have it just to show we know how to deal with the data. even though we do a lot of decoding and
/// cursor through raw bytes, its safe because we know the format of the data
///
/// span batch format (https://specs.optimism.io/protocol/delta/span-batches.html#future-batch-format-extension):
///
/// ```text
/// SpanBatch = SpanBatchPrefix ++ SpanBatchPayload
///
/// SpanBatchPrefix:
///   rel_timestamp:    varint    <- skip
///   l1_origin_num:    varint    <- skip
///   parent_check:     20 bytes  <- skip
///   l1_origin_check:  20 bytes  <- skip
///
/// SpanBatchPayload:
///   block_count:      varint    <- extract this
///   origin_bits:      bitfield  <- skip (ceil(block_count/8) bytes)
///   block_tx_counts:  [varint]  <- sum these (one per block)
///   txs:              ...       <- skip (transaction data)
/// ```
fn decode_span_batch_summary(data: &[u8]) -> Option<(u64, u64)> {
    let mut cursor = data;

    // --- SpanBatchPrefix ---
    // rel_timestamp (varint) - relative timestamp of first block
    let (_, rest) = unsigned_varint::decode::u64(cursor).ok()?;
    cursor = rest;

    // l1_origin_num (varint) - L1 origin block number
    let (_, rest) = unsigned_varint::decode::u64(cursor).ok()?;
    cursor = rest;

    // parent_check (20 bytes) + l1_origin_check (20 bytes)
    if cursor.len() < 40 {
        return None;
    }
    cursor = &cursor[40..];

    // --- SpanBatchPayload ---
    // block_count (varint) - number of L2 blocks in this batch
    let (block_count, rest) = unsigned_varint::decode::u64(cursor).ok()?;
    cursor = rest;

    if block_count == 0 || block_count > 10_000_000 {
        return None;
    }

    // origin_bits (bitfield) - one bit per block indicating L1 origin change
    let bits_len = (block_count as usize + 7) / 8;
    if cursor.len() < bits_len {
        return None;
    }
    cursor = &cursor[bits_len..];

    // block_tx_counts (varint per block) - transaction count for each L2 block
    let mut total_tx_count = 0u64;
    for _ in 0..block_count {
        let (tx_count, rest) = unsigned_varint::decode::u64(cursor).ok()?;
        total_tx_count += tx_count;
        cursor = rest;
    }

    Some((block_count, total_tx_count))
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
