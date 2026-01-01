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
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone)]
struct UnichainBatchTracker {
    pub batches_processed: u64,
    pub total_blobs: u64,
    pub frames_decoded: u64,
    pub batches: Vec<BatchTransaction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct BatchTransaction {
    pub tx_hash: B256,
    pub block_number: u64,
    pub block_timestamp: u64,
    pub block_hash: B256,
    pub from: Address,
    pub to: Address,
    pub tx_type: u8,
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
            batches = %self.batches_processed,
            blobs = %self.total_blobs,
            frames = %self.frames_decoded,
            "stats"
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
    let _pipeline = DerivationPipeline::new(config.clone());
    let mut tracker = UnichainBatchTracker::new();
    let mut blocks_processed: u64 = 0;

    while let Some(notification) = ctx.notifications.try_next().await? {
        match &notification {
            ExExNotification::ChainCommitted { new: chain } => {
                tracing::debug!(target: "derivexex::exex", chain = ?chain.range(), "chain committed");
                blocks_processed += chain.blocks().len() as u64;

                let batch_txs: Vec<BatchTransaction> = chain
                    .blocks_iter()
                    .flat_map(|block| {
                        let block_number = block.number();
                        let block_timestamp = block.timestamp();
                        let block_hash = block.hash();

                        block.transactions_with_sender().filter_map(move |(sender, tx)| {
                            let to = tx.to()?;

                            if to != batch_inbox || *sender != expected_batcher {
                                return None;
                            }

                            let blob_hashes: Vec<B256> =
                                tx.blob_versioned_hashes().map(|h| h.to_vec()).unwrap_or_default();

                            Some(BatchTransaction {
                                tx_hash: *tx.tx_hash(),
                                block_number,
                                block_timestamp,
                                block_hash,
                                from: *sender,
                                to,
                                tx_type: tx.ty(),
                                blob_count: blob_hashes.len(),
                                blob_hashes,
                            })
                        })
                    })
                    .collect();

                for batch in batch_txs {
                    tracing::debug!(
                        target: "derivexex::exex",
                        tx = %batch.tx_hash,
                        blobs = %batch.blob_count,
                        "processing batch"
                    );

                    let _slot = config.timestamp_to_slot(batch.block_timestamp);

                    tracker.total_blobs += batch.blob_count as u64;
                    tracker.batches_processed += 1;
                    tracker.batches.push(batch);
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
            }
            ExExNotification::ChainReverted { old } => {
                tracing::debug!(target: "derivexex::exex", chain = ?old.range(), "chain reverted");
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
