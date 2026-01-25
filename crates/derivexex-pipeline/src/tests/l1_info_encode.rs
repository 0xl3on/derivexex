use crate::l1_info::{
    encode::{encode_ecotone, encode_isthmus, ECOTONE_SELECTOR, ISTHMUS_SELECTOR},
    L1BlockInfo, ECOTONE_L1_INFO_TX_CALLDATA_LEN, ISTHMUS_L1_INFO_TX_CALLDATA_LEN,
};
use alloy_primitives::{address, B256, U256};

fn test_info() -> L1BlockInfo {
    L1BlockInfo {
        number: 0x123456789ABCDEF0,
        timestamp: 0xFEDCBA9876543210,
        basefee: U256::from(0xAABBCCDDu64),
        hash: B256::repeat_byte(0x42),
        sequence_number: 0x1111222233334444,
        batcher_addr: address!("1234567890123456789012345678901234567890"),
        blob_basefee: U256::from(0x55667788u64),
        basefee_scalar: 0xAABBCCDD,
        blob_basefee_scalar: 0x11223344,
        operator_fee_scalar: 0xDEADBEEF,
        operator_fee_constant: 0xCAFEBABE12345678,
    }
}

#[test]
fn test_ecotone_encoding_length() {
    let info = test_info();
    let mut buf = Vec::new();
    encode_ecotone(&info, &mut buf);

    assert_eq!(buf.len(), ECOTONE_L1_INFO_TX_CALLDATA_LEN);
    assert_eq!(buf.len(), 164);
}

#[test]
fn test_ecotone_selector() {
    let info = test_info();
    let mut buf = Vec::new();
    encode_ecotone(&info, &mut buf);

    assert_eq!(&buf[0..4], &ECOTONE_SELECTOR);
}

#[test]
fn test_ecotone_field_positions() {
    let info = test_info();
    let mut buf = Vec::new();
    encode_ecotone(&info, &mut buf);

    // baseFeeScalar at offset 4
    assert_eq!(&buf[4..8], &info.basefee_scalar.to_be_bytes());

    // blobBaseFeeScalar at offset 8
    assert_eq!(&buf[8..12], &info.blob_basefee_scalar.to_be_bytes());

    // sequenceNumber at offset 12
    assert_eq!(&buf[12..20], &info.sequence_number.to_be_bytes());

    // timestamp at offset 20
    assert_eq!(&buf[20..28], &info.timestamp.to_be_bytes());

    // number at offset 28
    assert_eq!(&buf[28..36], &info.number.to_be_bytes());

    // hash at offset 100
    assert_eq!(&buf[100..132], info.hash.as_slice());

    // batcherHash at offset 132 (address in first 20 bytes)
    assert_eq!(&buf[132..152], info.batcher_addr.as_slice());
    assert_eq!(&buf[152..164], &[0u8; 12]); // padding
}

#[test]
fn test_isthmus_encoding_length() {
    let info = test_info();
    let mut buf = Vec::new();
    encode_isthmus(&info, &mut buf);

    assert_eq!(buf.len(), ISTHMUS_L1_INFO_TX_CALLDATA_LEN);
    assert_eq!(buf.len(), 180);
}

#[test]
fn test_isthmus_selector() {
    let info = test_info();
    let mut buf = Vec::new();
    encode_isthmus(&info, &mut buf);

    assert_eq!(&buf[0..4], &ISTHMUS_SELECTOR);
}

#[test]
fn test_isthmus_operator_fee_fields() {
    let info = test_info();
    let mut buf = Vec::new();
    encode_isthmus(&info, &mut buf);

    // operatorFeeScalar at offset 164
    assert_eq!(&buf[164..168], &info.operator_fee_scalar.to_be_bytes());

    // operatorFeeConstant at offset 168
    assert_eq!(&buf[168..176], &info.operator_fee_constant.to_be_bytes());

    // padding at offset 176
    assert_eq!(&buf[176..180], &[0u8; 4]);
}

#[test]
fn test_buffer_reuse() {
    let info = test_info();
    let mut buf = Vec::with_capacity(512);

    encode_ecotone(&info, &mut buf);
    let first = buf.clone();

    buf.clear();
    encode_ecotone(&info, &mut buf);

    assert_eq!(buf, first);
}

#[test]
fn test_encodings_deterministic() {
    let info = test_info();

    let mut buf1 = Vec::new();
    let mut buf2 = Vec::new();

    encode_ecotone(&info, &mut buf1);
    encode_ecotone(&info, &mut buf2);
    assert_eq!(buf1, buf2);

    buf1.clear();
    buf2.clear();
    encode_isthmus(&info, &mut buf1);
    encode_isthmus(&info, &mut buf2);
    assert_eq!(buf1, buf2);
}
