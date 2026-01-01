use crate::{BATCH_INBOX, init, unichain_batch_exex};
use alloy_consensus::TxLegacy;
use alloy_eips::eip7685::Requests;
use alloy_primitives::{Address, B256, TxKind};
use eyre::Result;
use reth::api::Block as _;
use reth::revm::db::BundleState;
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_exex_test_utils::{PollOnce, TestExExHandle, test_exex_context};
use reth_primitives::{Block, BlockBody, Header, Receipt, RecoveredBlock, Transaction, TxType};
use reth_testing_utils::generators::{rng, sign_tx_with_random_key_pair};
use std::future::Future;
use std::pin::pin;
use std::sync::Once;

static INIT: Once = Once::new();

fn init_tracing() {
    INIT.call_once(|| {
        tracing_subscriber::fmt()
            .with_env_filter("derivexex=debug,info")
            .with_test_writer()
            .try_init()
            .ok();
    });
}

async fn setup_exex() -> Result<(TestExExHandle, impl Future<Output = Result<()>>)> {
    init_tracing();
    let (ctx, handle) = test_exex_context().await?;
    let exex = init(ctx).await?;
    Ok((handle, exex))
}

async fn setup_exex_with_batcher(
    batcher: Address,
) -> Result<(TestExExHandle, impl Future<Output = Result<()>>)> {
    init_tracing();
    let (ctx, handle) = test_exex_context().await?;
    let exex = unichain_batch_exex(ctx, batcher);
    Ok((handle, exex))
}

use helpers::*;

#[tokio::test]
async fn test_exex_detects_batch_tx() -> Result<()> {
    let (handle, exex) = setup_exex().await?;
    let mut exex = pin!(exex);

    let (tx, receipt) = build_batch_tx()?;
    let chain = build_single_block_chain(vec![tx], vec![receipt]);

    handle.send_notification_chain_committed(chain).await?;
    exex.poll_once().await?;

    Ok(())
}

#[tokio::test]
async fn test_exex_ignores_non_batch_tx() -> Result<()> {
    let (handle, exex) = setup_exex().await?;
    let mut exex = pin!(exex);

    let (tx, receipt) = build_random_tx()?;
    let chain = build_single_block_chain(vec![tx], vec![receipt]);

    handle.send_notification_chain_committed(chain).await?;
    exex.poll_once().await?;

    Ok(())
}

#[tokio::test]
async fn test_exex_handles_multiple_blocks() -> Result<()> {
    let (handle, exex) = setup_exex().await?;
    let mut exex = pin!(exex);

    let (batch_tx1, receipt1) = build_batch_tx()?;
    let (other_tx, receipt2) = build_random_tx()?;
    let (batch_tx2, receipt3) = build_batch_tx()?;

    let block1 = build_block(1, vec![batch_tx1]);
    let block2 = build_block(2, vec![other_tx, batch_tx2]);

    let chain = build_chain(
        vec![block1, block2],
        vec![vec![receipt1], vec![receipt2, receipt3]],
    );

    handle.send_notification_chain_committed(chain).await?;
    exex.poll_once().await?;

    Ok(())
}

#[tokio::test]
async fn test_exex_handles_revert() -> Result<()> {
    let (handle, exex) = setup_exex().await?;
    let mut exex = pin!(exex);

    let (tx, receipt) = build_batch_tx()?;
    let chain = build_single_block_chain(vec![tx], vec![receipt]);

    handle
        .send_notification_chain_committed(chain.clone())
        .await?;
    exex.poll_once().await?;

    handle.send_notification_chain_reverted(chain).await?;
    exex.poll_once().await?;

    Ok(())
}

#[tokio::test]
async fn test_exex_valid_batcher_with_blobs() -> Result<()> {
    let (tx, receipt, sender) = build_blob_batch_tx(3)?;

    let (handle, exex) = setup_exex_with_batcher(sender).await?;
    let mut exex = pin!(exex);

    let chain = build_single_block_chain(vec![tx], vec![receipt]);

    handle.send_notification_chain_committed(chain).await?;
    exex.poll_once().await?;

    Ok(())
}

mod helpers {
    use super::*;

    pub fn build_batch_tx() -> Result<(reth_primitives::TransactionSigned, Receipt)> {
        let tx = Transaction::Legacy(TxLegacy {
            to: TxKind::Call(BATCH_INBOX),
            ..Default::default()
        });

        #[allow(clippy::needless_update)]
        let receipt = Receipt {
            tx_type: TxType::Legacy,
            success: true,
            cumulative_gas_used: 21000,
            logs: vec![],
            ..Default::default()
        };

        Ok((sign_tx_with_random_key_pair(&mut rng(), tx), receipt))
    }

    pub fn build_blob_batch_tx(
        blob_count: usize,
    ) -> Result<(reth_primitives::TransactionSigned, Receipt, Address)> {
        use alloy_consensus::TxEip4844;
        use alloy_eips::eip4844::BYTES_PER_BLOB;
        use alloy_network::TxSignerSync;
        use alloy_signer_local::PrivateKeySigner;

        let signer = PrivateKeySigner::random();
        let sender_address = signer.address();

        let blob_versioned_hashes: Vec<B256> = (0..blob_count)
            .map(|i| {
                let mut hash = B256::random();
                hash.0[0] = 0x01;
                hash.0[1] = i as u8;
                hash
            })
            .collect();

        let mut tx = TxEip4844 {
            chain_id: 1,
            nonce: 0,
            gas_limit: 100_000,
            max_fee_per_gas: 20_000_000_000,
            max_priority_fee_per_gas: 1_000_000_000,
            max_fee_per_blob_gas: 1_000_000_000,
            to: BATCH_INBOX,
            value: alloy_primitives::U256::ZERO,
            input: alloy_primitives::Bytes::new(),
            access_list: Default::default(),
            blob_versioned_hashes,
        };

        let signature = signer.sign_transaction_sync(&mut tx)?;
        let signed_tx =
            reth_primitives::TransactionSigned::new_unhashed(Transaction::Eip4844(tx), signature);

        #[allow(clippy::needless_update)]
        let receipt = Receipt {
            tx_type: TxType::Eip4844,
            success: true,
            cumulative_gas_used: 21000 + (BYTES_PER_BLOB * blob_count) as u64,
            logs: vec![],
            ..Default::default()
        };

        Ok((signed_tx, receipt, sender_address))
    }

    pub fn build_random_tx() -> Result<(reth_primitives::TransactionSigned, Receipt)> {
        let tx = Transaction::Legacy(TxLegacy {
            to: TxKind::Call(Address::random()),
            ..Default::default()
        });

        #[allow(clippy::needless_update)]
        let receipt = Receipt {
            tx_type: TxType::Legacy,
            success: true,
            cumulative_gas_used: 21000,
            logs: vec![],
            ..Default::default()
        };

        Ok((sign_tx_with_random_key_pair(&mut rng(), tx), receipt))
    }

    pub fn build_block(
        number: u64,
        txs: Vec<reth_primitives::TransactionSigned>,
    ) -> RecoveredBlock<Block> {
        Block {
            header: Header {
                number,
                ..Default::default()
            },
            body: BlockBody {
                transactions: txs,
                ..Default::default()
            },
        }
        .seal_slow()
        .try_recover()
        .expect("failed to recover block")
    }

    pub fn build_chain(
        blocks: Vec<RecoveredBlock<Block>>,
        receipts_per_block: Vec<Vec<Receipt>>,
    ) -> Chain {
        let first_block_number = blocks.first().map(|b| b.number).unwrap_or(0);
        let requests = vec![Requests::default(); blocks.len()];

        Chain::new(
            blocks,
            ExecutionOutcome::new(
                BundleState::default(),
                receipts_per_block,
                first_block_number,
                requests,
            ),
            None,
        )
    }

    pub fn build_single_block_chain(
        txs: Vec<reth_primitives::TransactionSigned>,
        receipts: Vec<Receipt>,
    ) -> Chain {
        let block = build_block(0, txs);
        build_chain(vec![block], vec![receipts])
    }
}
