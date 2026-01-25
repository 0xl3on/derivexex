use crate::deposits::{DepositedTransaction, DEPOSIT_TX_TYPE, TRANSACTION_DEPOSITED_TOPIC};
use alloy_primitives::{keccak256, Address, Bytes, B256, U256};

#[test]
fn test_source_hash_computation() {
    use crate::deposits::parse::compute_source_hash;

    let block_hash = B256::repeat_byte(0x01);
    let log_index = 5u64;

    let deposit = DepositedTransaction {
        source_hash: compute_source_hash(block_hash, log_index),
        from: Address::ZERO,
        to: Some(Address::ZERO),
        mint: U256::ZERO,
        value: U256::ZERO,
        gas_limit: 21000,
        is_system_tx: false,
        data: Bytes::new(),
    };

    assert_ne!(deposit.source_hash, B256::ZERO);
}

#[test]
fn test_deposit_event_topic() {
    let expected = keccak256("TransactionDeposited(address,address,uint256,bytes)");
    assert_eq!(TRANSACTION_DEPOSITED_TOPIC, expected);
}

#[test]
fn test_encode_roundtrip_buffer_reuse() {
    let deposit = DepositedTransaction {
        source_hash: B256::repeat_byte(0xab),
        from: Address::repeat_byte(0x01),
        to: Some(Address::repeat_byte(0x02)),
        mint: U256::from(1000),
        value: U256::from(500),
        gas_limit: 100_000,
        is_system_tx: false,
        data: Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
    };

    let mut buf = Vec::new();
    deposit.encode(&mut buf);

    assert_eq!(buf[0], DEPOSIT_TX_TYPE);
    assert!(!buf.is_empty());

    // Reuse buffer
    buf.clear();
    deposit.encode(&mut buf);
    assert_eq!(buf[0], DEPOSIT_TX_TYPE);
}
