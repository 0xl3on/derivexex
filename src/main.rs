mod config;
mod pipeline;
mod providers;

use alloy_consensus::{BlockHeader, Typed2718};
use alloy_primitives::{Address, B256};
use config::UnichainConfig;
use futures::Future;
use futures_util::TryStreamExt;
use pipeline::DerivationPipeline;
use providers::PoolBeaconBlobProvider;
use reth::{
    api::FullNodeComponents, builder::NodeTypes, primitives::EthPrimitives,
    rpc::types::TransactionTrait,
};
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing;
use std::sync::Arc;

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

    // Create combined blob provider: tries pool first (fast), falls back to beacon API
    let blob_provider =
        PoolBeaconBlobProvider::new(Arc::new(ctx.pool().clone()), &config.beacon_url);
    let mut pipeline = DerivationPipeline::new(config.clone(), blob_provider);

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
    use pipeline::Batch;

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
