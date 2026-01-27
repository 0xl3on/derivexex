//! Encoding for L1 block info calldata (Ecotone and Isthmus).
//!
//! ## Ecotone (packed, 164 bytes)
//! <https://specs.optimism.io/protocol/ecotone/l1-attributes.html>
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
//! <https://specs.optimism.io/protocol/isthmus/l1-attributes.html>
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
