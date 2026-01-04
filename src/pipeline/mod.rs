mod batch;
mod channel;
mod frame;

pub use batch::Batch;
#[cfg(test)]
pub use batch::SpanBatch;
pub use channel::{Channel, ChannelAssembler};
pub use frame::{ChannelFrame, FrameDecoder};

use crate::providers::{BlobProvider, BlobProviderError};
use alloy_eips::eip4844::Blob;
use alloy_primitives::B256;

#[derive(thiserror::Error, Debug)]
pub enum PipelineError {
    #[error("Blob error: {0}")]
    Blob(#[from] BlobProviderError),
    #[error("Frame decode error: {0}")]
    FrameDecode(String),
}

pub struct DerivationPipeline<B: BlobProvider> {
    blob_provider: B,
    assembler: ChannelAssembler,
    /// Reusable buffer for blob decoding to avoid 130KB allocation per blob
    blob_decode_buf: Vec<u8>,
}

impl<B: BlobProvider> DerivationPipeline<B> {
    pub fn new(blob_provider: B) -> Self {
        Self {
            blob_provider,
            assembler: ChannelAssembler::new(),
            // Pre-allocate the decode buffer once, reused for all blobs
            blob_decode_buf: vec![0u8; BLOB_MAX_DATA_SIZE],
        }
    }

    pub async fn process_blobs(
        &mut self,
        slot: u64,
        blob_hashes: &[B256],
    ) -> Result<Vec<ChannelFrame>, PipelineError> {
        // frames are the contents of the blobs as discussed in the READMe.
        // so we will decode each blob into frames and then add them to the assembler to form
        // channels.
        let mut frames = Vec::new();

        for hash in blob_hashes {
            match self.blob_provider.get_blob(slot, *hash).await {
                Ok(blob) => {
                    let blob_frames = self.decode_blob(&blob)?;
                    frames.extend(blob_frames);
                }
                Err(e) => {
                    tracing::warn!(target: "derivexex::pipeline", hash = %hash, error = %e, "failed to fetch blob");
                }
            }
        }

        for frame in &frames {
            // add the frame to the assembler to form channels.
            // we know what frames belong to the which channel because the frame contains the
            // channel id.
            self.assembler.add_frame(frame.clone());
        }

        Ok(frames)
    }

    fn decode_blob(&mut self, blob: &Blob) -> Result<Vec<ChannelFrame>, PipelineError> {
        let len = decode_blob_data_into(blob, &mut self.blob_decode_buf);
        FrameDecoder::decode_frames(&self.blob_decode_buf[..len])
            .map_err(|e| PipelineError::FrameDecode(e.to_string()))
    }

    #[inline]
    pub fn take_complete_channels(&mut self) -> Vec<Channel> {
        self.assembler.take_complete()
    }
}

/// Maximum blob data size: (4 * 31 + 3) * 1024 - 4 = 130044 bytes
const BLOB_MAX_DATA_SIZE: usize = 130044;

/// Number of encoding rounds (1024 field element groups of 4)
const BLOB_ENCODING_ROUNDS: usize = 1024;

/// Decodes blob data from EIP-4844 field element encoding (OP v0 format) into provided buffer.
/// Returns the length of decoded data. Buffer must be at least BLOB_MAX_DATA_SIZE bytes.
/// https://github.com/op-rs/kona/blob/fe6dfcf771059109f1d75043d5ecbbfa3b6ca1a5/crates/protocol/derive/src/sources/blob_data.rs#L29
fn decode_blob_data_into(blob: &Blob, output: &mut [u8]) -> usize {
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
#[allow(dead_code)]
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
