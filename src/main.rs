use alloy_consensus::{BlockHeader, Typed2718};
use alloy_primitives::{address, Address, B256};
use eyre::Result;
use futures::Future;
use futures_util::TryStreamExt;
use reth::{
    api::FullNodeComponents, builder::NodeTypes, primitives::EthPrimitives,
    rpc::types::TransactionTrait,
};
use reth_exex::{ExExContext, ExExEvent, ExExNotification};
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing;
use serde::{Deserialize, Serialize};

const BATCH_INBOX: Address = address!("Ff00000000000000000000000000000000000130");
const BATCHER: Address = address!("2F60A5184c63ca94f82a27100643DbAbe4F3f7Fd");

struct Deriver<Node: FullNodeComponents> {
    exex_ctx: ExExContext<Node>,
    expected_batcher: Address,
    tracker: UnichainBatchTracker,
}

#[derive(Debug, Default, Clone)]
struct UnichainBatchTracker {
    pub batches_processed: u64,
    pub total_blobs: u64,
    pub batches: Vec<BatchTransaction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct BatchTransaction {
    pub tx_hash: B256,
    pub block_number: u64,
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
            "stats"
        );
    }
}

pub async fn init<Node>(ctx: ExExContext<Node>) -> Result<impl Future<Output = Result<()>>>
where
    Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
{
    Ok(unichain_batch_exex(ctx, BATCHER))
}

pub(crate) async fn unichain_batch_exex<Node>(
    mut ctx: ExExContext<Node>,
    expected_batcher: Address,
) -> eyre::Result<()>
where
    Node: FullNodeComponents<Types: NodeTypes<Primitives = EthPrimitives>>,
{
    let mut tracker = UnichainBatchTracker::new();
    let mut blocks_processed: u64 = 0;

    while let Some(notification) = ctx.notifications.try_next().await? {
        match &notification {
            ExExNotification::ChainCommitted { new } => {
                tracing::debug!(target: "derivexex::exex", chain = ?new.range(), "chain committed");
                blocks_processed += new.blocks().len() as u64;

                for block in new.blocks_iter() {
                    let block_number = block.number();
                    let block_hash = block.hash();

                    for (sender, tx) in block.transactions_with_sender() {
                        let Some(to_address) = tx.to() else { continue };
                        if to_address != BATCH_INBOX {
                            continue;
                        }

                        let from_address = *sender;
                        let tx_type = tx.ty();
                        let blob_hashes: Vec<B256> =
                            tx.blob_versioned_hashes().map(|h| h.to_vec()).unwrap_or_default();
                        let blob_count = blob_hashes.len();

                        if from_address != expected_batcher {
                            tracing::warn!(
                                target: "derivexex::exex",
                                sender = %from_address,
                                expected = %expected_batcher,
                                "unexpected batcher"
                            );
                        } else {
                            tracing::debug!(target: "derivexex::exex", sender = %from_address, "valid batch");
                        }
                        tracker.batches_processed += 1;
                        tracker.total_blobs += blob_count as u64;
                        tracker.batches.push(BatchTransaction {
                            tx_hash: *tx.tx_hash(),
                            block_number,
                            block_hash,
                            from: from_address,
                            to: to_address,
                            tx_type,
                            blob_count,
                            blob_hashes,
                        });
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
            }
            ExExNotification::ChainReverted { old } => {
                tracing::debug!(target: "derivexex::exex", chain = ?old.range(), "chain reverted");
            }
        }

        if let Some(committed_chain) = notification.committed_chain() {
            // this is crucial so that Reth knows what blocks have been processed
            // and pruning can happen
            ctx.events.send(ExExEvent::FinishedHeight(committed_chain.tip().num_hash()))?;
        }
    }

    tracing::info!(target: "derivexex::exex", "shutting down");
    tracker.log_stats();

    Ok(())
}

fn main() -> Result<()> {
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
