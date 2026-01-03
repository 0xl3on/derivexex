//! Common test helpers and utilities.
use crate::config::{UnichainConfig, BATCHER, BATCH_INBOX, L2_CHAIN_ID};
use alloy_consensus::TxLegacy;
use alloy_eips::eip7685::Requests;
use alloy_primitives::{Address, TxKind, B256};
use eyre::Result;
use reth::{api::Block as _, revm::db::BundleState};
use reth_execution_types::{Chain, ExecutionOutcome};
use reth_primitives::{Block, BlockBody, Header, Receipt, RecoveredBlock, Transaction, TxType};
use reth_testing_utils::generators::{rng, sign_tx_with_random_key_pair};
use std::{env, sync::Once};

static INIT: Once = Once::new();

pub fn init_test_env() {
    INIT.call_once(|| {
        dotenvy::dotenv().ok();
        tracing_subscriber::fmt()
            .with_env_filter("derivexex=debug,info")
            .with_test_writer()
            .try_init()
            .ok();
    });
}

pub fn test_config() -> UnichainConfig {
    init_test_env();
    let beacon_url = env::var("BEACON_API").expect("BEACON_API missing");
    UnichainConfig {
        batch_inbox: BATCH_INBOX,
        batcher: BATCHER,
        l2_chain_id: L2_CHAIN_ID,
        beacon_url,
        beacon_genesis_time: 1606824023,
        l1_seconds_per_slot: 12,
    }
}

pub fn build_batch_tx() -> Result<(reth_primitives::TransactionSigned, Receipt)> {
    let tx = Transaction::Legacy(TxLegacy { to: TxKind::Call(BATCH_INBOX), ..Default::default() });

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
    let tx =
        Transaction::Legacy(TxLegacy { to: TxKind::Call(Address::random()), ..Default::default() });

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
        header: Header { number, ..Default::default() },
        body: BlockBody { transactions: txs, ..Default::default() },
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
