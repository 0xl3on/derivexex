//! Encoding for L1 block info calldata (Ecotone and Isthmus).
//!
//! ## Ecotone (packed, 164 bytes)
//! https://specs.optimism.io/protocol/ecotone/l1-attributes.html
//! ```text
//! | Offset | Size | Field              |
//! |--------|------|--------------------|
//! | 0      | 4    | Function selector  |
//! | 4      | 4    | baseFeeScalar      |
//! | 8      | 4    | blobBaseFeeScalar  |
//! | 12     | 8    | sequenceNumber     |
//! | 20     | 8    | l1BlockTimestamp   |
//! | 28     | 8    | l1BlockNumber      |
//! | 36     | 32   | basefee            |
//! | 68     | 32   | blobBaseFee        |
//! | 100    | 32   | l1BlockHash        |
//! | 132    | 32   | batcherHash        |
//! ```
//!
//! ## Isthmus (packed, 180 bytes)
//! Ecotone layout + 16 bytes for operator fees:
//! https://specs.optimism.io/protocol/isthmus/l1-attributes.html
//! ```text
//! | 164    | 4    | operatorFeeScalar   |
//! | 168    | 8    | operatorFeeConstant |
//! | 176    | 4    | padding (zeros)     |
//! ```

use super::L1BlockInfo;

// === Function selectors ===

/// Ecotone: setL1BlockValuesEcotone()
pub const ECOTONE_SELECTOR: [u8; 4] = [0x44, 0x0a, 0x5e, 0x20];

/// Isthmus: setL1BlockValuesIsthmus()
pub const ISTHMUS_SELECTOR: [u8; 4] = [0x09, 0x8f, 0x40, 0x81];

// === Calldata lengths ===

/// Ecotone: 4 + 4 + 4 + 8 + 8 + 8 + 32 + 32 + 32 + 32 = 164 bytes
pub const ECOTONE_L1_INFO_TX_CALLDATA_LEN: usize = 164;

/// Isthmus: Ecotone (164) + 4 + 8 + 4 = 180 bytes
pub const ISTHMUS_L1_INFO_TX_CALLDATA_LEN: usize = 180;

/// Encode L1BlockInfo for Ecotone hardfork (packed).
///
/// All integers are big-endian with sizes matching their types.
pub fn encode_ecotone(info: &L1BlockInfo, out: &mut Vec<u8>) {
    out.reserve(ECOTONE_L1_INFO_TX_CALLDATA_LEN);

    // Function selector (4 bytes)
    out.extend_from_slice(&ECOTONE_SELECTOR);

    // baseFeeScalar (4 bytes)
    out.extend_from_slice(&info.basefee_scalar.to_be_bytes());

    // blobBaseFeeScalar (4 bytes)
    out.extend_from_slice(&info.blob_basefee_scalar.to_be_bytes());

    // sequenceNumber (8 bytes)
    out.extend_from_slice(&info.sequence_number.to_be_bytes());

    // l1BlockTimestamp (8 bytes)
    out.extend_from_slice(&info.timestamp.to_be_bytes());

    // l1BlockNumber (8 bytes)
    out.extend_from_slice(&info.number.to_be_bytes());

    // basefee (32 bytes)
    out.extend_from_slice(&info.basefee.to_be_bytes::<32>());

    // blobBaseFee (32 bytes)
    out.extend_from_slice(&info.blob_basefee.to_be_bytes::<32>());

    // l1BlockHash (32 bytes)
    out.extend_from_slice(info.hash.as_slice());

    // batcherHash (32 bytes, address in first 20 bytes, zero-padded)
    let mut batcher_hash = [0u8; 32];
    batcher_hash[..20].copy_from_slice(info.batcher_addr.as_slice());
    out.extend_from_slice(&batcher_hash);

    debug_assert_eq!(out.len(), ECOTONE_L1_INFO_TX_CALLDATA_LEN);
}

/// Encode L1BlockInfo for Isthmus hardfork (packed).
///
/// Same as Ecotone, plus operator fee fields.
pub fn encode_isthmus(info: &L1BlockInfo, out: &mut Vec<u8>) {
    out.reserve(ISTHMUS_L1_INFO_TX_CALLDATA_LEN);

    // Function selector (4 bytes)
    out.extend_from_slice(&ISTHMUS_SELECTOR);

    // baseFeeScalar (4 bytes)
    out.extend_from_slice(&info.basefee_scalar.to_be_bytes());

    // blobBaseFeeScalar (4 bytes)
    out.extend_from_slice(&info.blob_basefee_scalar.to_be_bytes());

    // sequenceNumber (8 bytes)
    out.extend_from_slice(&info.sequence_number.to_be_bytes());

    // l1BlockTimestamp (8 bytes)
    out.extend_from_slice(&info.timestamp.to_be_bytes());

    // l1BlockNumber (8 bytes)
    out.extend_from_slice(&info.number.to_be_bytes());

    // basefee (32 bytes)
    out.extend_from_slice(&info.basefee.to_be_bytes::<32>());

    // blobBaseFee (32 bytes)
    out.extend_from_slice(&info.blob_basefee.to_be_bytes::<32>());

    // l1BlockHash (32 bytes)
    out.extend_from_slice(info.hash.as_slice());

    // batcherHash (32 bytes, address in first 20 bytes, zero-padded)
    let mut batcher_hash = [0u8; 32];
    batcher_hash[..20].copy_from_slice(info.batcher_addr.as_slice());
    out.extend_from_slice(&batcher_hash);

    // operatorFeeScalar (4 bytes)
    out.extend_from_slice(&info.operator_fee_scalar.to_be_bytes());

    // operatorFeeConstant (8 bytes)
    out.extend_from_slice(&info.operator_fee_constant.to_be_bytes());

    // padding (4 bytes of zeros for 32-byte alignment)
    out.extend_from_slice(&[0u8; 4]);

    debug_assert_eq!(out.len(), ISTHMUS_L1_INFO_TX_CALLDATA_LEN);
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
