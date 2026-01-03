//! ExEx unit tests, these are just basic stuff like fetching chain commits, blocks, txs, etc.
use super::helpers::*;
use crate::{config::UnichainConfig, unichain_batch_exex};
use alloy_primitives::Address;
use eyre::Result;
use reth_exex_test_utils::{test_exex_context, PollOnce, TestExExHandle};
use std::{future::Future, pin::pin};

async fn setup_exex() -> Result<(TestExExHandle, impl Future<Output = Result<()>>)> {
    let (ctx, handle) = test_exex_context().await?;
    let config = test_config();
    let exex = unichain_batch_exex(ctx, config);
    Ok((handle, exex))
}

async fn setup_exex_with_batcher(
    batcher: Address,
) -> Result<(TestExExHandle, impl Future<Output = Result<()>>)> {
    let (ctx, handle) = test_exex_context().await?;
    let config = UnichainConfig { batcher, ..test_config() };
    let exex = unichain_batch_exex(ctx, config);
    Ok((handle, exex))
}

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

    let chain = build_chain(vec![block1, block2], vec![vec![receipt1], vec![receipt2, receipt3]]);

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

    handle.send_notification_chain_committed(chain.clone()).await?;
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
