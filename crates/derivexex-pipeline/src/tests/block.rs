use crate::{
    block::{BlockBuildError, L1BlockRef, L2BlockBuilder},
    deposits::DepositedTransaction,
    l1_info::{Hardfork, L1BlockInfo},
};
use alloy_primitives::{address, Bytes, B256, U256};

fn mock_l1_origin() -> L1BlockRef {
    L1BlockRef { number: 21000000, hash: B256::repeat_byte(0x11), timestamp: 1700000000 }
}

fn mock_l1_info(sequence_number: u64) -> L1BlockInfo {
    L1BlockInfo::new(
        21000000,
        1700000000,
        U256::from(30_000_000_000u64), // 30 gwei
        B256::repeat_byte(0x11),
        sequence_number,
        address!("2F60A5184c63ca94f82a27100643DbAbe4F3f7Fd"),
        U256::from(1u64),
        1000,
        500,
    )
}

fn mock_deposit() -> DepositedTransaction {
    DepositedTransaction {
        source_hash: B256::repeat_byte(0xab),
        from: address!("1111111111111111111111111111111111111111"),
        to: Some(address!("2222222222222222222222222222222222222222")),
        mint: U256::from(1_000_000_000_000_000_000u64), // 1 ETH
        value: U256::from(1_000_000_000_000_000_000u64),
        gas_limit: 100_000,
        is_system_tx: false,
        data: Bytes::new(),
    }
}

#[test]
fn test_build_first_block_with_deposits() {
    let mut builder = L2BlockBuilder::new(Hardfork::Ecotone);
    builder.set_l1_origin(mock_l1_origin());
    builder.add_deposits([mock_deposit(), mock_deposit()]);

    let l1_info = mock_l1_info(0);
    let sequencer_txs = vec![Bytes::from_static(&[0x01, 0x02, 0x03])];

    let block = builder.build_block(1700000002, &l1_info, sequencer_txs).unwrap();

    assert_eq!(block.number, 0); // Starts at 0
    assert_eq!(block.timestamp, 1700000002);
    assert_eq!(block.sequence_number, 0);
    assert!(block.is_epoch_start());

    // L1 info + 2 deposits + 1 sequencer tx
    assert_eq!(block.tx_count(), 4);
    assert_eq!(block.deposit_count(), 3); // L1 info + 2 user deposits
    assert_eq!(block.sequencer_tx_count(), 1);
}

#[test]
fn test_subsequent_blocks_no_deposits() {
    let mut builder = L2BlockBuilder::new(Hardfork::Ecotone);
    builder.set_l1_origin(mock_l1_origin());
    builder.add_deposits([mock_deposit()]);

    // First block gets the deposit
    let block1 = builder.build_block(1700000002, &mock_l1_info(0), vec![]).unwrap();
    assert_eq!(block1.deposit_count(), 2); // L1 info + 1 deposit

    // Second block in same epoch - no deposits
    let block2 = builder.build_block(1700000004, &mock_l1_info(1), vec![]).unwrap();
    assert_eq!(block2.deposit_count(), 1); // Only L1 info
    assert_eq!(block2.sequence_number, 1);
    assert!(!block2.is_epoch_start());
}

#[test]
fn test_new_epoch_resets_sequence_number() {
    let mut builder = L2BlockBuilder::new(Hardfork::Ecotone);

    // First epoch
    builder.set_l1_origin(mock_l1_origin());
    let _ = builder.build_block(1700000002, &mock_l1_info(0), vec![]).unwrap();
    let _ = builder.build_block(1700000004, &mock_l1_info(1), vec![]).unwrap();
    assert_eq!(builder.current_sequence_number(), 2);

    // New epoch
    builder.set_l1_origin(L1BlockRef {
        number: 21000001,
        hash: B256::repeat_byte(0x22),
        timestamp: 1700000012,
    });
    assert_eq!(builder.current_sequence_number(), 0);

    let block = builder.build_block(1700000014, &mock_l1_info(0), vec![]).unwrap();
    assert_eq!(block.sequence_number, 0);
    assert!(block.is_epoch_start());
}

#[test]
fn test_error_without_l1_origin() {
    let mut builder = L2BlockBuilder::new(Hardfork::Ecotone);

    let result = builder.build_block(1700000002, &mock_l1_info(0), vec![]);
    assert!(matches!(result, Err(BlockBuildError::NoL1Origin)));
}

#[test]
fn test_block_number_starts_at_zero() {
    let mut builder = L2BlockBuilder::new(Hardfork::Ecotone);
    builder.set_l1_origin(mock_l1_origin());

    assert_eq!(builder.current_block_number(), 0);

    let block1 = builder.build_block(1700000001, &mock_l1_info(0), vec![]).unwrap();
    assert_eq!(block1.number, 0);
    assert_eq!(builder.current_block_number(), 1);

    let block2 = builder.build_block(1700000002, &mock_l1_info(1), vec![]).unwrap();
    assert_eq!(block2.number, 1);
    assert_eq!(builder.current_block_number(), 2);
}

#[test]
fn test_set_block_number_for_resume() {
    let mut builder = L2BlockBuilder::new(Hardfork::Ecotone);
    builder.set_block_number(5000); // Resume from checkpoint
    builder.set_l1_origin(mock_l1_origin());

    assert_eq!(builder.current_block_number(), 5000);

    let block1 = builder.build_block(1700000001, &mock_l1_info(0), vec![]).unwrap();
    assert_eq!(block1.number, 5000);
    assert_eq!(builder.current_block_number(), 5001);

    let block2 = builder.build_block(1700000002, &mock_l1_info(1), vec![]).unwrap();
    assert_eq!(block2.number, 5001);
    assert_eq!(builder.current_block_number(), 5002);
}

#[test]
fn test_l1_origin_in_block() {
    let mut builder = L2BlockBuilder::new(Hardfork::Ecotone);
    let origin = mock_l1_origin();
    builder.set_l1_origin(origin.clone());

    let block = builder.build_block(1700000001, &mock_l1_info(0), vec![]).unwrap();

    assert_eq!(block.l1_origin.number, origin.number);
    assert_eq!(block.l1_origin.hash, origin.hash);
    assert_eq!(block.l1_origin.timestamp, origin.timestamp);
}
