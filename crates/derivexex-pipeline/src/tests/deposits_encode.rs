use crate::deposits::{
    encode::{encode_deposit_tx, tx_hash, DEPOSIT_TX_TYPE},
    DepositedTransaction,
};
use alloy_primitives::{address, Address, Bytes, B256, U256};

#[test]
fn test_trim_leading_zeros() {
    use crate::deposits::encode::trim_leading_zeros;

    assert_eq!(trim_leading_zeros(&[0, 0, 0, 1]), &[1]);
    assert_eq!(trim_leading_zeros(&[0, 0, 1, 0]), &[1, 0]);
    assert_eq!(trim_leading_zeros(&[1, 2, 3]), &[1, 2, 3]);
}

#[test]
fn test_encode_starts_with_type() {
    let tx = DepositedTransaction {
        source_hash: B256::ZERO,
        from: Address::ZERO,
        to: Some(Address::ZERO),
        mint: U256::ZERO,
        value: U256::ZERO,
        gas_limit: 21000,
        is_system_tx: false,
        data: Bytes::new(),
    };

    let mut buf = Vec::new();
    encode_deposit_tx(&tx, &mut buf);

    assert_eq!(buf[0], DEPOSIT_TX_TYPE);
}

#[test]
fn test_contract_creation_empty_to() {
    let tx = DepositedTransaction {
        source_hash: B256::repeat_byte(0x11),
        from: address!("1111111111111111111111111111111111111111"),
        to: None, // Contract creation
        mint: U256::from(1000),
        value: U256::ZERO,
        gas_limit: 100_000,
        is_system_tx: false,
        data: Bytes::from_static(&[0x60, 0x80, 0x60, 0x40]), // Sample bytecode
    };

    let mut buf = Vec::new();
    encode_deposit_tx(&tx, &mut buf);

    // Should encode successfully
    assert!(buf.len() > 1);

    // Hash should be deterministic
    let hash1 = tx_hash(&tx);
    let hash2 = tx_hash(&tx);
    assert_eq!(hash1, hash2);
}
