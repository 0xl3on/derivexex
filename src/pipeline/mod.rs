mod channel;
mod frame;

pub use channel::{Channel, ChannelAssembler};
pub use frame::{ChannelFrame, FrameDecoder};

use crate::{config::UnichainConfig, providers::BeaconBlobProvider};
use alloy_eips::eip4844::Blob;
use alloy_primitives::B256;
use std::sync::Arc;

#[derive(thiserror::Error, Debug)]
pub enum PipelineError {
    #[error("Beacon error: {0}")]
    Beacon(#[from] crate::providers::beacon::BeaconError),
    #[error("Frame decode error: {0}")]
    FrameDecode(String),
    #[error("Channel error: {0}")]
    Channel(String),
}

pub struct DerivationPipeline {
    config: Arc<UnichainConfig>,
    beacon: BeaconBlobProvider,
    assembler: ChannelAssembler,
}

impl DerivationPipeline {
    pub fn new(config: UnichainConfig) -> Self {
        let beacon = BeaconBlobProvider::new(&config.beacon_url);

        Self { config: Arc::new(config), beacon, assembler: ChannelAssembler::new() }
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
            match self.beacon.get_blob_by_hash(slot, *hash).await {
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

    fn decode_blob(&self, blob: &Blob) -> Result<Vec<ChannelFrame>, PipelineError> {
        let data = decode_blob_data(blob);

        FrameDecoder::decode_frames(&data).map_err(|e| PipelineError::FrameDecode(e.to_string()))
    }

    #[inline]
    pub fn take_complete_channels(&mut self) -> Vec<Channel> {
        self.assembler.take_complete()
    }

    #[inline]
    pub fn config(&self) -> &UnichainConfig {
        &self.config
    }
}

fn decode_blob_data(blob: &Blob) -> Vec<u8> {
    let mut data = Vec::with_capacity(blob.len());

    for chunk in blob.chunks(32) {
        if chunk.len() >= 32 {
            data.extend_from_slice(&chunk[1..32]);
        }
    }

    if let Some(end) = data.iter().position(|&b| b == 0x80) {
        data.truncate(end);
    }

    data
}
