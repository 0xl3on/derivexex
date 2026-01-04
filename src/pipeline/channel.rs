//! Channel assembly and decompression.
//!
//! - `Channel`: a fully assembled channel ready for decompression. contains concatenated frame
//!   data.
//! - `PendingChannel`: buffers frames until all arrive. tracks which frames we have and if we've
//!   seen the last one.
//! - `ChannelAssembler`: routes incoming frames to their pending channels and yields complete ones.
use super::frame::ChannelFrame;
use std::{collections::HashMap, io::Read};

type ChannelId = [u8; 16];

// zlib has a very specific header format:
/// the lower nibble is always 8, which is the compression method DEFLATE
/// common first bytes for zlib are: 0x78, 0x68, 0x58, all have lower nibble = 8
const ZLIB_DEFLATE_METHOD: u8 = 8;

/// brotli doesnt have a nice header format, so in the op stack, they use a version byte prefix.
const BROTLI_VERSION: u8 = 1;

#[derive(thiserror::Error, Debug)]
pub enum ChannelError {
    #[error("Decompression failed: {0}")]
    Decompression(String),
    #[error("Empty channel data")]
    Empty,
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub id: ChannelId,
    pub data: Vec<u8>,
    #[allow(dead_code)]
    pub frame_count: usize,
}

impl Channel {
    /// - zlib: lower nibble == 8 (4 bits)(deflate method)
    /// - brotli: first byte == 1
    pub fn decompress(&self) -> Result<Vec<u8>, ChannelError> {
        if self.data.is_empty() {
            return Err(ChannelError::Empty);
        }

        let first = self.data[0];

        if first == BROTLI_VERSION {
            self.decompress_brotli()
        } else if (first & 0x0F) == ZLIB_DEFLATE_METHOD {
            // here we use a bit mask to check if the lower nibble is 8 because 0x0F is 0000 1111
            // so if we do the & operation, we will get the lower 4 bits of the first byte!
            self.decompress_zlib()
        } else {
            Err(ChannelError::Decompression(format!("unknown compression type: {}", first)))
        }
    }

    fn decompress_zlib(&self) -> Result<Vec<u8>, ChannelError> {
        // zlib: decompress entire data (header included)
        miniz_oxide::inflate::decompress_to_vec_zlib(&self.data)
            .map_err(|e| ChannelError::Decompression(format!("zlib: {:?}", e)))
    }

    fn decompress_brotli(&self) -> Result<Vec<u8>, ChannelError> {
        // brotli: skip version byte (first byte)
        let mut decoder = brotli::Decompressor::new(&self.data[1..], 4096);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out).map_err(|e| ChannelError::Decompression(e.to_string()))?;
        Ok(out)
    }
}

struct PendingChannel {
    // the u16 key is the frame number, the value is the frame.
    frames: HashMap<u16, ChannelFrame>,
    // the highest frame number in the channel.
    highest_frame: u16,
    // we know a channel is complete when it's last frame is received.
    is_closed: bool,
}

impl PendingChannel {
    fn new() -> Self {
        Self { frames: HashMap::new(), highest_frame: 0, is_closed: false }
    }

    fn add_frame(&mut self, frame: ChannelFrame) {
        // we can know a channel is closed when the last frame is received
        if frame.is_last {
            self.is_closed = true;
        }
        // update the highest frame number in the channel.
        self.highest_frame = self.highest_frame.max(frame.frame_number);
        // add the frame to the frames map.
        // the key is the frame number, the value is the frame.
        self.frames.insert(frame.frame_number, frame);
    }

    fn is_complete(&self) -> bool {
        // a channel is not complete if it's not closed.
        if !self.is_closed {
            return false;
        }

        // a channel is complete if all frames are received.
        for i in 0..=self.highest_frame {
            if !self.frames.contains_key(&i) {
                return false;
            }
        }
        true
    }

    fn assemble(self) -> Channel {
        // assemble the channel data by concatenating the frames.
        let mut data = Vec::new();
        let id = self.frames.values().next().map(|f| f.channel_id).unwrap_or([0; 16]);

        let mut frame_nums: Vec<_> = self.frames.keys().copied().collect();
        frame_nums.sort();

        // sort the frame numbers to ensure the frames are in order.
        for num in &frame_nums {
            if let Some(frame) = self.frames.get(num) {
                data.extend_from_slice(&frame.frame_data);
            }
        }

        Channel { id, data, frame_count: frame_nums.len() }
    }
}

pub struct ChannelAssembler {
    pending: HashMap<ChannelId, PendingChannel>,
}

impl ChannelAssembler {
    pub fn new() -> Self {
        Self { pending: HashMap::new() }
    }

    #[inline]
    pub fn add_frame(&mut self, frame: ChannelFrame) {
        self.pending.entry(frame.channel_id).or_insert_with(PendingChannel::new).add_frame(frame);
    }

    pub fn take_complete(&mut self) -> Vec<Channel> {
        let complete_ids: Vec<_> =
            self.pending.iter().filter(|(_, ch)| ch.is_complete()).map(|(id, _)| *id).collect();

        complete_ids
            .into_iter()
            .filter_map(|id| self.pending.remove(&id).map(|ch| ch.assemble()))
            .collect()
    }
}

impl Default for ChannelAssembler {
    fn default() -> Self {
        Self::new()
    }
}
