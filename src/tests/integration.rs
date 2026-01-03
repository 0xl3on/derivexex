//! Integration tests using real blob data.

use super::{fixtures::get_or_fetch_blob, helpers::*};
use crate::{
    pipeline::{decode_blob_data, ChannelAssembler, FrameDecoder},
    providers::BeaconBlobProvider,
};
use alloy_primitives::B256;
use eyre::Result;

/// integration test using real blob data from mainnet.
/// tx: 0x713d467f3bdef6e2328f3daafd4f5bfbc827b3db63eec6af7c6f161663f8131e
/// BLOCK: 24149392, TIMESTAMP: 1767386303 (Jan-02-2026 20:38:23 UTC)
/// blobs are cached to local file to survive beacon pruning (~18 days).
#[tokio::test]
async fn test_frame_decoder_with_real_blobs() -> Result<()> {
    init_test_env();
    let config = test_config();
    let beacon = BeaconBlobProvider::new(&config.beacon_url);

    let block_timestamp: u64 = 1767386303;
    let slot = config.timestamp_to_slot(block_timestamp);

    let versioned_hashes: [B256; 3] = [
        "0x010d628a3a0dfa67dcef83700e3112944385031479b9ee30b767448f23eec5b4".parse()?,
        "0x01cdba5028140db2b9c051371c8101b449177da4b3e4b06e0e44d1fb8843b156".parse()?,
        "0x01066ce0dde5a705b8a856c9c4c4df7ec371149dc54e5c9151a4959b0bc0de9d".parse()?,
    ];

    let mut assembler = ChannelAssembler::new();
    let mut total_frames = 0;

    for hash in &versioned_hashes {
        let blob = get_or_fetch_blob(&beacon, slot, *hash).await?;
        let data = decode_blob_data(&blob);

        if data.is_empty() || data[0] != 0x00 {
            continue;
        }

        let frames = match FrameDecoder::decode_frames(&data) {
            Ok(f) => f,
            Err(_) => continue,
        };

        if !frames.is_empty() {
            tracing::info!(hash = %hash, frames = frames.len(), "decoded frames");
        }

        for frame in frames {
            total_frames += 1;
            tracing::debug!(
                channel_id = ?frame.channel_id,
                frame_number = frame.frame_number,
                is_last = frame.is_last,
                "frame"
            );
            assembler.add_frame(frame);
        }
    }

    let complete_channels = assembler.take_complete();

    tracing::info!(
        total_frames = total_frames,
        complete_channels = complete_channels.len(),
        "pipeline results"
    );

    assert!(total_frames > 0, "expected to decode at least one frame");

    Ok(())
}
