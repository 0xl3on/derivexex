//! OP Stack derivation pipeline for decoding blobs, frames, channels, and batches.
//!
//! This crate provides the core derivation logic for processing OP Stack data:
//! - Blob decoding (EIP-4844 field element encoding)
//! - Frame parsing (extracting frames from blob data)
//! - Channel assembly (combining frames into complete channels)
//! - Batch decoding (SingleBatch and SpanBatch formats)
//! - L2 block building with deposits

mod batch;
mod block;
mod channel;
mod derive;
mod frame;

pub mod deposits;
pub mod l1_info;

#[cfg(test)]
mod tests;

pub use batch::{Batch, BatchError, SingleBatch, SpanBatch, SpanBatchElement};
pub use block::{BlockBuildError, L1BlockRef, L2Block, L2BlockBuilder, L2Transaction};
pub use channel::{Channel, ChannelAssembler, ChannelError};
pub use deposits::{
    DepositError, DepositedTransaction, DEPOSIT_TX_TYPE, TRANSACTION_DEPOSITED_TOPIC,
};
pub use derive::{ChannelResult, DeriveError, Deriver, DeriverConfig, EpochInfo};
pub use frame::{ChannelFrame, FrameDecoder, FrameError};
pub use l1_info::{
    compute_l1_info_source_hash, Hardfork, L1BlockInfo, ECOTONE_L1_INFO_TX_CALLDATA_LEN,
    ISTHMUS_L1_INFO_TX_CALLDATA_LEN, L1_ATTRIBUTES_DEPOSITOR, L1_BLOCK_ADDRESS, L1_INFO_TX_GAS,
};

use alloy_eips::eip4844::Blob;

/// Maximum blob data size: (4 * 31 + 3) * 1024 - 4 = 130044 bytes
const BLOB_MAX_DATA_SIZE: usize = 130044;

/// Number of encoding rounds (1024 field element groups of 4)
const BLOB_ENCODING_ROUNDS: usize = 1024;

/// Decodes blob data from EIP-4844 field element encoding (OP v0 format) into provided buffer.
/// Returns the length of decoded data. Buffer must be at least BLOB_MAX_DATA_SIZE bytes.
/// https://github.com/op-rs/kona/blob/fe6dfcf771059109f1d75043d5ecbbfa3b6ca1a5/crates/protocol/derive/src/sources/blob_data.rs#L29
pub fn decode_blob_data_into(blob: &Blob, output: &mut [u8]) -> usize {
    let data: &[u8] = blob.as_ref();

    // Version at byte 1 (VERSIONED_HASH_VERSION_KZG position)
    if data.len() < 32 || data[1] != 0x00 {
        return 0;
    }

    // Length from bytes 2-4 (3 bytes big-endian)
    let length = u32::from_be_bytes([0, data[2], data[3], data[4]]) as usize;
    if length > BLOB_MAX_DATA_SIZE || output.len() < BLOB_MAX_DATA_SIZE {
        return 0;
    }

    // Round 0: copy first 27 bytes from data[5..32]
    output[0..27].copy_from_slice(&data[5..32]);

    let mut output_pos = 28;
    let mut input_pos = 32;
    let mut encoded_byte = [0u8; 4];
    encoded_byte[0] = data[0];

    // Process remaining 3 field elements of round 0
    for b in encoded_byte.iter_mut().skip(1) {
        if let Some((enc, opos, ipos)) = decode_field_element(data, output_pos, input_pos, output) {
            *b = enc;
            output_pos = opos;
            input_pos = ipos;
        } else {
            return 0;
        }
    }

    // Reassemble 4x6-bit encoded chunks into 3 bytes
    output_pos = reassemble_bytes(output_pos, &encoded_byte, output);

    // Remaining rounds: decode 4 field elements (128 bytes) into 127 bytes
    for _ in 1..BLOB_ENCODING_ROUNDS {
        if output_pos >= length {
            break;
        }

        for d in &mut encoded_byte {
            if let Some((enc, opos, ipos)) =
                decode_field_element(data, output_pos, input_pos, output)
            {
                *d = enc;
                output_pos = opos;
                input_pos = ipos;
            } else {
                return 0;
            }
        }
        output_pos = reassemble_bytes(output_pos, &encoded_byte, output);
    }

    length
}

/// Decodes blob data from EIP-4844 field element encoding (OP v0 format).
/// Allocates a new buffer - prefer `decode_blob_data_into` for hot paths.
pub fn decode_blob_data(blob: &Blob) -> Vec<u8> {
    let mut output = vec![0u8; BLOB_MAX_DATA_SIZE];
    let len = decode_blob_data_into(blob, &mut output);
    output.truncate(len);
    output
}

/// Decodes a field element: copies 31 bytes to output, returns the encoded high byte.
/// the decoding logic was mostly taken from kona's BlobData::decode implementation
/// https://github.com/op-rs/kona/blob/fe6dfcf771059109f1d75043d5ecbbfa3b6ca1a5/crates/protocol/derive/src/sources/blob_data.rs#L106
fn decode_field_element(
    data: &[u8],
    output_pos: usize,
    input_pos: usize,
    output: &mut [u8],
) -> Option<(u8, usize, usize)> {
    if input_pos + 32 > data.len() || output_pos + 31 > output.len() {
        return None;
    }

    // Two highest bits of first byte must be 0
    if data[input_pos] & 0b1100_0000 != 0 {
        return None;
    }

    output[output_pos..output_pos + 31].copy_from_slice(&data[input_pos + 1..input_pos + 32]);
    Some((data[input_pos], output_pos + 32, input_pos + 32))
}

/// Reassembles 4x6-bit encoded chunks into 3 bytes of output.
/// the reassembling logic was mostly taken from kona's BlobData::decode implementation
/// https://github.com/op-rs/kona/blob/fe6dfcf771059109f1d75043d5ecbbfa3b6ca1a5/crates/protocol/derive/src/sources/blob_data.rs#L126
fn reassemble_bytes(mut output_pos: usize, encoded_byte: &[u8; 4], output: &mut [u8]) -> usize {
    output_pos -= 1;
    let x = (encoded_byte[0] & 0b0011_1111) | ((encoded_byte[1] & 0b0011_0000) << 2);
    let y = (encoded_byte[1] & 0b0000_1111) | ((encoded_byte[3] & 0b0000_1111) << 4);
    let z = (encoded_byte[2] & 0b0011_1111) | ((encoded_byte[3] & 0b0011_0000) << 2);
    output[output_pos - 32] = z;
    output[output_pos - 32 * 2] = y;
    output[output_pos - 32 * 3] = x;
    output_pos
}

/// Returns the maximum blob data size constant.
pub const fn max_blob_data_size() -> usize {
    BLOB_MAX_DATA_SIZE
}
