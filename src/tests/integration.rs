//! Integration tests using real blob data.

use super::{fixtures::get_or_fetch_blob, helpers::*};
use crate::{
    pipeline::{decode_blob_data, ChannelAssembler, FrameDecoder},
    providers::BeaconBlobProvider,
};
use alloy_primitives::{Bytes, B256};
use alloy_rlp::Decodable;
use eyre::Result;

/// Decoded span batch information
struct SpanBatchInfo {
    rel_timestamp: u64,
    l1_origin_num: u64,
    block_count: u64,
    block_tx_counts: Vec<u64>,
    total_tx_count: u64,
    /// Remaining data after header (contains transactions)
    tx_data_offset: usize,
}

/// Decoded transaction summary
struct TxSummary {
    contract_creations: usize,
    regular_txs: usize,
}

/// Decodes a span batch prefix and payload to extract block/tx counts
fn decode_span_batch(data: &[u8]) -> Option<SpanBatchInfo> {
    let original_len = data.len();
    let mut cursor = data;

    // Prefix: rel_timestamp, l1_origin_num, parent_check (20), l1_origin_check (20)
    let (rel_timestamp, rest) = unsigned_varint::decode::u64(cursor).ok()?;
    cursor = rest;

    let (l1_origin_num, rest) = unsigned_varint::decode::u64(cursor).ok()?;
    cursor = rest;

    // Skip parent_check and l1_origin_check (40 bytes total)
    if cursor.len() < 40 {
        return None;
    }
    cursor = &cursor[40..];

    // Payload: block_count
    let (block_count, rest) = unsigned_varint::decode::u64(cursor).ok()?;
    cursor = rest;

    if block_count == 0 || block_count > 10_000_000 {
        return None;
    }

    // origin_bits: ceil(block_count / 8) bytes
    let bits_len = (block_count as usize + 7) / 8;
    if cursor.len() < bits_len {
        return None;
    }
    cursor = &cursor[bits_len..];

    // block_tx_counts: one varint per block
    let mut block_tx_counts = Vec::with_capacity(block_count as usize);
    for _ in 0..block_count {
        let (tx_count, rest) = unsigned_varint::decode::u64(cursor).ok()?;
        block_tx_counts.push(tx_count);
        cursor = rest;
    }

    let total_tx_count: u64 = block_tx_counts.iter().sum();
    let tx_data_offset = original_len - cursor.len();

    Some(SpanBatchInfo {
        rel_timestamp,
        l1_origin_num,
        block_count,
        block_tx_counts,
        total_tx_count,
        tx_data_offset,
    })
}

/// Decoded transaction info
#[derive(Debug)]
struct DecodedTx {
    index: usize,
    is_contract_creation: bool,
    to: Option<[u8; 20]>,
    nonce: u64,
    gas_limit: u64,
    sig_r: [u8; 32],
    sig_s: [u8; 32],
    y_parity: bool,
}

/// Decode transactions from span batch data
fn decode_transactions(data: &[u8], total_tx_count: u64) -> Option<Vec<DecodedTx>> {
    let total = total_tx_count as usize;
    let mut cursor = data;

    // 1. contract_creation_bits: ceil(total_tx_count / 8) bytes
    let cc_bits_len = (total + 7) / 8;
    if cursor.len() < cc_bits_len {
        return None;
    }
    let cc_bits = &cursor[..cc_bits_len];
    cursor = &cursor[cc_bits_len..];

    // Parse contract creation flags
    let mut is_contract_creation = vec![false; total];
    let mut contract_count = 0usize;
    for i in 0..total {
        let byte_idx = i / 8;
        let bit_idx = i % 8;
        if cc_bits[byte_idx] & (1 << (7 - bit_idx)) != 0 {
            is_contract_creation[i] = true;
            contract_count += 1;
        }
    }
    let regular_count = total - contract_count;

    // 2. y_parity_bits: ceil(total_tx_count / 8) bytes
    let y_bits_len = (total + 7) / 8;
    if cursor.len() < y_bits_len {
        return None;
    }
    let y_bits = &cursor[..y_bits_len];
    cursor = &cursor[y_bits_len..];

    // 3. signatures: 64 bytes (r + s) per tx
    let sigs_len = total * 64;
    if cursor.len() < sigs_len {
        return None;
    }
    let sigs_data = &cursor[..sigs_len];
    cursor = &cursor[sigs_len..];

    // 4. tx_tos: 20 bytes per non-contract-creation tx
    let tos_len = regular_count * 20;
    if cursor.len() < tos_len {
        return None;
    }
    let tos_data = &cursor[..tos_len];
    cursor = &cursor[tos_len..];

    // Skip tx_data (variable length, complex to parse)
    // 5. tx_nonces: varints
    // 6. tx_gases: varints
    // We need to skip tx_data first, which is tricky. For now, let's just decode what we have.

    // Build decoded transactions (without nonces/gas for now)
    let mut txs = Vec::with_capacity(total);
    let mut to_idx = 0usize;

    for i in 0..total {
        let byte_idx = i / 8;
        let bit_idx = i % 8;
        let y_parity = y_bits[byte_idx] & (1 << (7 - bit_idx)) != 0;

        let mut sig_r = [0u8; 32];
        let mut sig_s = [0u8; 32];
        sig_r.copy_from_slice(&sigs_data[i * 64..i * 64 + 32]);
        sig_s.copy_from_slice(&sigs_data[i * 64 + 32..i * 64 + 64]);

        let to = if is_contract_creation[i] {
            None
        } else {
            let mut addr = [0u8; 20];
            addr.copy_from_slice(&tos_data[to_idx * 20..(to_idx + 1) * 20]);
            to_idx += 1;
            Some(addr)
        };

        txs.push(DecodedTx {
            index: i,
            is_contract_creation: is_contract_creation[i],
            to,
            nonce: 0,     // Would need to parse tx_data first
            gas_limit: 0, // Would need to parse tx_data first
            sig_r,
            sig_s,
            y_parity,
        });
    }

    Some(txs)
}

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

                    // Decode span batch (type=1)
                    if batch_type == 1 {
                        if let Some(span_info) = decode_span_batch(&batch_bytes[1..]) {
                            tracing::info!(
                                rel_timestamp = span_info.rel_timestamp,
                                l1_origin_num = span_info.l1_origin_num,
                                block_count = span_info.block_count,
                                total_txs = span_info.total_tx_count,
                                "span batch decoded"
                            );

                            // Decode transactions
                            let tx_data = &batch_bytes[1 + span_info.tx_data_offset..];
                            if let Some(txs) =
                                decode_transactions(tx_data, span_info.total_tx_count)
                            {
                                let contract_count =
                                    txs.iter().filter(|t| t.is_contract_creation).count();
                                tracing::info!(
                                    total = txs.len(),
                                    contract_creations = contract_count,
                                    regular_txs = txs.len() - contract_count,
                                    "decoded transactions"
                                );

                                // Show first 5 transactions
                                for tx in txs.iter().take(5) {
                                    let to_str = tx
                                        .to
                                        .map(|a| hex::encode(a))
                                        .unwrap_or_else(|| "CREATE".to_string());
                                    tracing::info!(
                                        idx = tx.index,
                                        to = %to_str,
                                        y_parity = tx.y_parity,
                                        sig_r = %hex::encode(&tx.sig_r[..8]),
                                        "tx"
                                    );
                                }

                                // Show contract creations
                                let creates: Vec<_> =
                                    txs.iter().filter(|t| t.is_contract_creation).collect();
                                if !creates.is_empty() {
                                    tracing::info!("contract creation transactions:");
                                    for tx in creates.iter().take(3) {
                                        tracing::info!(
                                            idx = tx.index,
                                            sig_r = %hex::encode(&tx.sig_r[..8]),
                                            "contract creation"
                                        );
                                    }
                                }
                            }

                            // Show tx counts per block (first 5)
                            for (i, tx_count) in
                                span_info.block_tx_counts.iter().take(5).enumerate()
                            {
                                tracing::debug!(block = i, tx_count = tx_count, "block tx count");
                            }
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
