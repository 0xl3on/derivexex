use crate::l1_info::{
    compute_l1_info_source_hash, Hardfork, L1BlockInfo, ECOTONE_L1_INFO_TX_CALLDATA_LEN,
    ISTHMUS_L1_INFO_TX_CALLDATA_LEN, L1_ATTRIBUTES_DEPOSITOR, L1_BLOCK_ADDRESS, L1_INFO_TX_GAS,
};
use alloy_primitives::{address, Address, B256, U256};

#[test]
fn test_hardfork_default_is_ecotone() {
    assert_eq!(Hardfork::default(), Hardfork::Ecotone);
}

#[test]
fn test_hardfork_selectors() {
    assert_eq!(Hardfork::Ecotone.l1_info_selector(), [0x44, 0x0a, 0x5e, 0x20]);
    assert_eq!(Hardfork::Isthmus.l1_info_selector(), [0x09, 0x8f, 0x40, 0x81]);
}

#[test]
fn test_hardfork_calldata_lengths() {
    assert_eq!(Hardfork::Ecotone.l1_info_calldata_len(), 164);
    assert_eq!(Hardfork::Isthmus.l1_info_calldata_len(), 180);
}

#[test]
fn test_detect_hardfork_from_calldata() {
    let info = L1BlockInfo::default();

    let ecotone_data = info.to_calldata(Hardfork::Ecotone);
    assert_eq!(Hardfork::from_l1_info_calldata(&ecotone_data), Some(Hardfork::Ecotone));

    let isthmus_data = info.to_calldata(Hardfork::Isthmus);
    assert_eq!(Hardfork::from_l1_info_calldata(&isthmus_data), Some(Hardfork::Isthmus));
}

#[test]
fn test_source_hash_uses_domain_1() {
    let hash = B256::repeat_byte(0xab);
    let seq = 5u64;

    let source_hash = compute_l1_info_source_hash(hash, seq);

    assert_ne!(source_hash, B256::ZERO);
    assert_eq!(source_hash, compute_l1_info_source_hash(hash, seq));

    // Different sequence number should give different hash
    let source_hash_2 = compute_l1_info_source_hash(hash, 6);
    assert_ne!(source_hash, source_hash_2);
}

#[test]
fn test_source_hash_verification_block_1008() {
    // This verifies the source_hash for block 1008 in the l2_blocks.json test output
    // l1_block_hash = 0x8a8a8a8a... (mock hash from test)
    // sequence_number = 8
    let l1_block_hash = B256::repeat_byte(0x8a);
    let sequence_number = 8u64;

    let source_hash = compute_l1_info_source_hash(l1_block_hash, sequence_number);

    // This is the expected source_hash from l2_blocks.json block 1008
    let expected: B256 =
        "0x2c7fadd35b795e92bbcce242429d49fa2adff2e189ce68e8c42cb72f95b1ca9b".parse().unwrap();

    assert_eq!(source_hash, expected, "source_hash should match JSON output");
}

#[test]
fn test_to_deposit_tx_ecotone() {
    let info = L1BlockInfo::new(
        12345,
        1700000000,
        U256::from(1_000_000_000u64),
        B256::repeat_byte(0x11),
        0,
        address!("2F60A5184c63ca94f82a27100643DbAbe4F3f7Fd"),
        U256::from(1u64),
        1000,
        500,
    );

    let deposit = info.to_deposit_tx(Hardfork::Ecotone);

    assert_eq!(deposit.from, L1_ATTRIBUTES_DEPOSITOR);
    assert_eq!(deposit.to, Some(L1_BLOCK_ADDRESS));
    assert_eq!(deposit.mint, U256::ZERO);
    assert_eq!(deposit.value, U256::ZERO);
    assert_eq!(deposit.gas_limit, L1_INFO_TX_GAS);
    assert!(deposit.is_system_tx);
    assert_eq!(deposit.data.len(), ECOTONE_L1_INFO_TX_CALLDATA_LEN);
}

#[test]
fn test_to_deposit_tx_isthmus() {
    let info = L1BlockInfo::new(
        12345,
        1700000000,
        U256::from(1_000_000_000u64),
        B256::repeat_byte(0x11),
        0,
        address!("2F60A5184c63ca94f82a27100643DbAbe4F3f7Fd"),
        U256::from(1u64),
        1000,
        500,
    )
    .with_operator_fees(100, 200);

    let deposit = info.to_deposit_tx(Hardfork::Isthmus);

    assert_eq!(deposit.data.len(), ISTHMUS_L1_INFO_TX_CALLDATA_LEN);
    assert_eq!(&deposit.data[..4], &[0x09, 0x8f, 0x40, 0x81]);
}

#[test]
fn test_builder_pattern() {
    let info =
        L1BlockInfo::new(1, 2, U256::ZERO, B256::ZERO, 0, Address::ZERO, U256::ZERO, 0, 0)
            .with_operator_fees(123, 456);

    assert_eq!(info.operator_fee_scalar, 123);
    assert_eq!(info.operator_fee_constant, 456);
}

#[test]
fn test_addresses() {
    assert_eq!(L1_ATTRIBUTES_DEPOSITOR, address!("DeaDDEaDDeAdDeAdDEAdDEaddeAddEAdDEAd0001"));
    assert_eq!(L1_BLOCK_ADDRESS, address!("4200000000000000000000000000000000000015"));
}
