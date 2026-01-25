//! Integration tests using real blob data.

use super::{fixtures::get_or_fetch_blob, helpers::*};
use crate::config::{BASE_FEE_SCALAR, BATCHER, BLOB_BASE_FEE_SCALAR};
use alloy_primitives::{Bytes, B256, U256};
use alloy_rlp::Decodable;
use derivexex_pipeline::{
    decode_blob_data, Batch, ChannelAssembler, Deriver, DeriverConfig, EpochInfo, FrameDecoder,
    Hardfork, L1BlockRef, SpanBatch,
};
use eyre::Result;

/// integration test using real blob data from mainnet.
/// tx: 0x713d467f3bdef6e2328f3daafd4f5bfbc827b3db63eec6af7c6f161663f8131e
/// BLOCK: 24149392, TIMESTAMP: 1767386303 (Jan-02-2026 20:38:23 UTC)
/// blobs are cached to local file to survive beacon pruning (~18 days).
#[tokio::test]
async fn test_frame_decoder_with_real_blobs() -> Result<()> {
    init_test_env();
    let config = test_config();
    let beacon = crate::providers::BeaconBlobProvider::new(&config.beacon_url);

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
    assert!(!complete_channels.is_empty(), "expected at least one complete channel");

    // Decompress and decode batches from the channel
    for channel in &complete_channels {
        let decompressed = channel.decompress()?;
        tracing::info!(
            channel_id = ?channel.id,
            compressed_size = channel.data.len(),
            decompressed_size = decompressed.len(),
            "decompressed channel"
        );

        // Decompressed data is RLP-encoded: decode to get raw batch bytes
        let mut cursor = decompressed.as_slice();
        let mut batch_count = 0;

        while !cursor.is_empty() {
            match Bytes::decode(&mut cursor) {
                Ok(batch_bytes) => {
                    batch_count += 1;
                    if batch_bytes.is_empty() {
                        continue;
                    }

                    let batch_type = batch_bytes[0];
                    tracing::info!(
                        batch_num = batch_count,
                        batch_type = batch_type,
                        batch_size = batch_bytes.len(),
                        "decoded batch (type: 0=single, 1=span)"
                    );

                    // Use our Kona-based Batch::decode
                    match Batch::decode(&batch_bytes) {
                        Ok(batch) => match batch {
                            Batch::Single(single) => {
                                tracing::info!(
                                    epoch_num = single.epoch_num,
                                    timestamp = single.timestamp,
                                    txs = single.transactions.len(),
                                    "single batch decoded"
                                );

                                for (i, tx) in single.transactions.iter().take(3).enumerate() {
                                    tracing::debug!(idx = i, tx_len = tx.len(), "single batch tx");
                                }
                            }
                            Batch::Span(span) => {
                                log_span_batch(&span);
                            }
                        },
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to decode batch");
                        }
                    }
                }
                Err(_) => break,
            }
        }

        tracing::info!(batch_count = batch_count, "total batches in channel");
    }

    Ok(())
}

/// Full integration test: blobs → frames → channels → batches → L2 blocks
/// Uses real cached blob data and the Deriver to build L2Block structs.
#[tokio::test]
async fn test_full_l2_block_building() -> Result<()> {
    init_test_env();
    let config = test_config();
    let beacon = crate::providers::BeaconBlobProvider::new(&config.beacon_url);

    // Same blob data as test_frame_decoder_with_real_blobs
    let block_timestamp: u64 = 1767386303;
    let slot = config.timestamp_to_slot(block_timestamp);

    let versioned_hashes: [B256; 3] = [
        "0x010d628a3a0dfa67dcef83700e3112944385031479b9ee30b767448f23eec5b4".parse()?,
        "0x01cdba5028140db2b9c051371c8101b449177da4b3e4b06e0e44d1fb8843b156".parse()?,
        "0x01066ce0dde5a705b8a856c9c4c4df7ec371149dc54e5c9151a4959b0bc0de9d".parse()?,
    ];

    // Step 1: Decode blobs into frames and assemble channels
    let mut assembler = ChannelAssembler::new();
    for hash in &versioned_hashes {
        let blob = get_or_fetch_blob(&beacon, slot, *hash).await?;
        let data = decode_blob_data(&blob);

        if data.is_empty() || data[0] != 0x00 {
            continue;
        }

        if let Ok(frames) = FrameDecoder::decode_frames(&data) {
            for frame in frames {
                assembler.add_frame(frame);
            }
        }
    }

    let complete_channels = assembler.take_complete();
    assert!(!complete_channels.is_empty(), "expected at least one complete channel");

    // Step 2: Create Deriver with config
    let deriver_config = DeriverConfig {
        hardfork: Hardfork::Ecotone,
        batcher_addr: BATCHER,
        base_fee_scalar: BASE_FEE_SCALAR,
        blob_base_fee_scalar: BLOB_BASE_FEE_SCALAR,
    };
    let mut deriver = Deriver::new(deriver_config, 1000); // Start at block 1000

    // Step 3: Register mock epoch info for the L1 blocks referenced in batches
    // Span batches can reference many epochs - 540 L2 blocks at 1s block time
    // spans ~45 L1 blocks (at 12s block time). Register a wide range.
    for epoch_num in 24149380..=24149450 {
        deriver.register_epoch(
            epoch_num,
            EpochInfo {
                l1_ref: L1BlockRef {
                    number: epoch_num,
                    hash: B256::repeat_byte((epoch_num & 0xFF) as u8),
                    timestamp: block_timestamp,
                },
                basefee: U256::from(30_000_000_000u64), // 30 gwei
                blob_basefee: U256::from(1u64),
            },
        );
    }

    // Step 4: Process channels through Deriver
    let mut total_l2_blocks = 0;
    let mut total_l2_txs = 0;

    for channel in &complete_channels {
        let result = deriver.process_channel(channel)?;

        total_l2_blocks += result.blocks_built;
        total_l2_txs += result.txs_count;

        tracing::info!(
            blocks_built = result.blocks_built,
            txs_count = result.txs_count,
            "processed channel"
        );

        // Verify block structure
        for block in &result.blocks {
            assert!(block.deposit_count() >= 1, "block should have at least L1 info deposit");
        }
    }

    tracing::info!(
        total_l2_blocks = total_l2_blocks,
        total_l2_txs = total_l2_txs,
        final_block_number = deriver.current_block_number(),
        "L2 block building complete"
    );

    // Verify we built some blocks
    assert!(total_l2_blocks > 0, "expected to build at least one L2 block");
    assert!(total_l2_txs > 0, "expected L2 blocks to have transactions");

    // Verify block numbers advanced correctly
    assert_eq!(
        deriver.current_block_number(),
        1000 + total_l2_blocks,
        "block number should advance by number of blocks built"
    );

    Ok(())
}

fn log_span_batch(span: &SpanBatch) {
    let total_txs: usize = span.blocks.iter().map(|b| b.transactions.len()).sum();

    tracing::info!(
        rel_timestamp = span.rel_timestamp,
        l1_origin_num = span.l1_origin_num,
        blocks = span.blocks.len(),
        total_txs = total_txs,
        parent_check = %hex::encode(&span.parent_check[..8]),
        l1_origin_check = %hex::encode(&span.l1_origin_check[..8]),
        "span batch decoded"
    );

    // Log first 5 blocks
    for (block_idx, element) in span.blocks.iter().take(5).enumerate() {
        tracing::info!(
            block_idx = block_idx,
            epoch_num = element.epoch_num,
            txs = element.transactions.len(),
            "span batch block"
        );

        // Log first 3 transactions in block
        for (tx_idx, tx) in element.transactions.iter().take(3).enumerate() {
            if !tx.is_empty() {
                let tx_type = tx[0];
                tracing::debug!(
                    block = block_idx,
                    tx = tx_idx,
                    tx_type = tx_type,
                    tx_len = tx.len(),
                    "span batch tx"
                );
            }
        }
    }

    // Summary stats
    let non_empty_blocks = span.blocks.iter().filter(|b| !b.transactions.is_empty()).count();
    let blocks_with_many_txs = span.blocks.iter().filter(|b| b.transactions.len() > 10).count();

    tracing::info!(
        total_blocks = span.blocks.len(),
        non_empty_blocks = non_empty_blocks,
        blocks_with_many_txs = blocks_with_many_txs,
        avg_txs_per_block = total_txs as f64 / span.blocks.len() as f64,
        "span batch summary"
    );
}
