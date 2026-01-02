//! Frame decoding from raw blob data.
//!
//! - `ChannelFrame`: a single frame containing a chunk of channel data. identified by channel_id
//!   and frame_number.
//! - `FrameDecoder`: parses blob data into frames. handles the derivation version byte and frame
//!   boundaries
use std::ops::Range;

#[derive(thiserror::Error, Debug)]
pub enum FrameError {
    #[error("Invalid frame: {0}")]
    Invalid(String),
    #[error("Incomplete frame data")]
    Incomplete,
}

#[derive(Debug, Clone)]
pub struct ChannelFrame {
    pub channel_id: [u8; 16],
    pub frame_number: u16,
    pub frame_data: Vec<u8>,
    pub is_last: bool,
}

/// as mentioned in the README, blobs contains frames that later will be assembled into channels.
pub struct FrameDecoder;

impl FrameDecoder {
    const DERIVATION_VERSION: u8 = 0x00;

    /// Frame layout offsets (https://specs.optimism.io/protocol/derivation.html#frame-format)
    const CHANNEL_ID: Range<usize> = 0..16;
    const FRAME_NUMBER: Range<usize> = 16..18;
    const FRAME_DATA_LENGTH: Range<usize> = 18..22;
    const IS_LAST: usize = 22;
    const HEADER_SIZE: usize = 23;

    pub fn decode_frames(data: &[u8]) -> Result<Vec<ChannelFrame>, FrameError> {
        if data.is_empty() {
            return Ok(Vec::new());
        }

        if data[0] != Self::DERIVATION_VERSION {
            return Err(FrameError::Invalid(format!("Unknown derivation version: {}", data[0])));
        }

        let mut frames = Vec::new();
        let mut offset = 1;

        while offset < data.len() {
            let (frame, consumed) = Self::decode_single_frame(&data[offset..])?;

            frames.push(frame);

            offset += consumed;
        }

        Ok(frames)
    }

    fn decode_single_frame(data: &[u8]) -> Result<(ChannelFrame, usize), FrameError> {
        if data.len() < Self::HEADER_SIZE {
            return Err(FrameError::Incomplete);
        }

        let mut channel_id = [0u8; 16];
        channel_id.copy_from_slice(&data[Self::CHANNEL_ID]);

        let frame_number =
            u16::from_be_bytes(data[Self::FRAME_NUMBER].try_into().expect("range is 2 bytes"));
        let frame_data_len =
            u32::from_be_bytes(data[Self::FRAME_DATA_LENGTH].try_into().expect("range is 4 bytes"))
                as usize;

        let total_size = Self::HEADER_SIZE + frame_data_len;

        if data.len() < total_size {
            return Err(FrameError::Incomplete);
        }

        let is_last = data[Self::IS_LAST] == 1;
        let frame_data = data[Self::HEADER_SIZE..total_size].to_vec();

        Ok((ChannelFrame { channel_id, frame_number, frame_data, is_last }, total_size))
    }
}
