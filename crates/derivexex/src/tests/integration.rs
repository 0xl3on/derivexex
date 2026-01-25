//! Integration tests using real blob data.

use super::{fixtures::get_or_fetch_blob, helpers::*};
use crate::config::{
    BASE_FEE_SCALAR, BATCHER, BLOB_BASE_FEE_SCALAR, L2_BLOCK_TIME, L2_GENESIS_TIME,
};
use alloy_eips::eip4844::calc_blob_gasprice;
use alloy_primitives::{Bytes, B256, U256};
use alloy_rlp::Decodable;
use derivexex_pipeline::{
    decode_blob_data, Batch, ChannelAssembler, Deriver, DeriverConfig, EpochInfo, FrameDecoder,
    Hardfork, L1BlockRef, L2Block, SpanBatch,
};
use eyre::Result;
use std::{fs::File, io::Write};

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
    // The span batch rel_timestamp = 36637400, which is the first L2 block number
    let starting_l2_block: u64 = 36637400;

    let deriver_config = DeriverConfig {
        hardfork: Hardfork::Ecotone,
        batcher_addr: BATCHER,
        base_fee_scalar: BASE_FEE_SCALAR,
        blob_base_fee_scalar: BLOB_BASE_FEE_SCALAR,
        l2_genesis_time: L2_GENESIS_TIME,
        l2_block_time: L2_BLOCK_TIME,
    };
    let mut deriver = Deriver::new(deriver_config, starting_l2_block);

    // Step 3: Register real L1 epoch info for the blocks referenced in the span batch.
    // These are real L1 block hashes and fees fetched from Ethereum mainnet.
    // The span batch references epochs 24149341-24149386 (46 L1 blocks).
    let real_l1_blocks: [(u64, &str, u64, u64, u64); 46] = [
        // (block_num, hash, timestamp, basefee, excess_blob_gas)
        (
            24149341,
            "0xad772c81fd6ab70747b10c0ff4270183fd1d86e1737a7e507b55a768eab127d1",
            0x69582a5b,
            0x64ad995,
            0x7c9741d,
        ),
        (
            24149342,
            "0x08ccf1f512ae5c87bc854a71147692630591493c128d4d351b6a9d316f68e726",
            0x69582a67,
            0x5fe09b8,
            0x7cec972,
        ),
        (
            24149343,
            "0x84de9eb64c46d08e911f4d39fde27e8caf60b2a6835f6e4b7c1956d2beb8ee23",
            0x69582a73,
            0x5dc91fc,
            0x7bec972,
        ),
        (
            24149344,
            "0xe38944dfeef578831dbdf3d62c011f459bb9a56063ead2ee09f6365aae2196b1",
            0x69582a7f,
            0x5d56616,
            0x7c3741c,
        ),
        (
            24149345,
            "0x45fcbd1b14c1aa109d41c685ad70c536eecd7f6ca9ccc3836f2df1b986f230f2",
            0x69582a8b,
            0x5cd2684,
            0x7c81ec6,
        ),
        (
            24149346,
            "0x11148e537c7630ee225267b782e4ad434e943133e6b5c8db6ce8cd204bfe21d2",
            0x69582a97,
            0x5fd2812,
            0x7bc1ec6,
        ),
        (
            24149347,
            "0xd7c3471488692d946f5b80d815bc094a2c7b91d7fe9c119a4981c855d1aa9c40",
            0x69582aa3,
            0x5db764d,
            0x7c0c970,
        ),
        (
            24149348,
            "0xb039f2b860f83579494393138628d1c7d2f5aa775ee848f0daa89460f5239731",
            0x69582aaf,
            0x5baa2e4,
            0x7c3741a,
        ),
        (
            24149349,
            "0x1572a019b402c749c4982cb6ac8975039cea34e67f5f107f82c52149c59e5922",
            0x69582abb,
            0x59729c6,
            0x7c4c96f,
        ),
        (
            24149350,
            "0x5057ac717d8fdb6fcfaa55b4a64808494bd33d5a696dcd13b2c4dfc85de8a69e",
            0x69582ac7,
            0x5850a35,
            0x7c4c96f,
        ),
        (
            24149351,
            "0xe9e6e0045382ac0ca2426c7fb778f913abf1b00f95dfcabc8fd617c63ecd34ba",
            0x69582ad3,
            0x57ffab0,
            0x7b2c96f,
        ),
        (
            24149352,
            "0x256dc899b655f6c13b57391eecdf870a5f4351395438d82d2132c49f3d9b5a79",
            0x69582adf,
            0x5a1e9b6,
            0x7b37419,
        ),
        (
            24149353,
            "0x2d04138779c33fba71665161a043419746fb290ce36ec49f204fc4767567af48",
            0x69582aeb,
            0x5681a25,
            0x7b41ec3,
        ),
        (
            24149354,
            "0x694f2c186417483579faec08e1ec6d9ce30a2f8fa658c759afd4e3a5607db6a7",
            0x69582af7,
            0x5d51e8a,
            0x7b81ec3,
        ),
        (
            24149355,
            "0x12c38d193c880cf3671cc611f443a2ed4c379ae92baac305a96d06103bc1e9bd",
            0x69582b03,
            0x5c6b6b7,
            0x7b97418,
        ),
        (
            24149356,
            "0xe42d40b1a05fb007384b200c79567054cf82548a5293ac1f8a1ddf5a7624040a",
            0x69582b0f,
            0x5b69b6e,
            0x7c21ec2,
        ),
        (
            24149357,
            "0xf52363d5ece282ae3d65bd4ab988fda410adb67d139a99a1f4b8ab1fd9ac2e88",
            0x69582b1b,
            0x51ab943,
            0x7c21ec2,
        ),
        (
            24149358,
            "0xc5559fc4232678c59209caff11d2ab689799f40b71e21c07648717bf3bd23a30",
            0x69582b27,
            0x5722304,
            0x7ae1ec2,
        ),
        (
            24149359,
            "0x4540e4ec32ce6d1936a6ca429631ed458f4c388a7cb3c2145b6e9036f324c93a",
            0x69582b33,
            0x56cf9ca,
            0x7aec96c,
        ),
        (
            24149360,
            "0x5f2656bb5a8348cc7dba1df79dc80c83817e952b7b84a134739d16a53d02e9f2",
            0x69582b3f,
            0x52ff73a,
            0x7b61ec1,
        ),
        (
            24149361,
            "0xf85f983ffec7fd4377b858230c1547fc038fbbfd2ab255db107d3b593d7a5880",
            0x69582b4b,
            0x531dbfe,
            0x7b8c96b,
        ),
        (
            24149362,
            "0x6eead5e23bdfc578fafbba6a802c59883e89555cbfee6dedaf0a585d68363464",
            0x69582b57,
            0x4e760e9,
            0x7a6c96b,
        ),
        (
            24149363,
            "0xdfcb743665b3458b9dca2565effcb10fdd670ba43ece1d171a5ccebd855194f1",
            0x69582b63,
            0x4fa0ccc,
            0x7ab7415,
        ),
        (
            24149364,
            "0x90a1b3732f72ee10d145234cfb033c1fa85c1951ab8274d5438eac75ee63a41d",
            0x69582b6f,
            0x50905b4,
            0x7ad7415,
        ),
        (
            24149365,
            "0xbc5c3909e9fb05a00f6c5d2dbb48e1a34758d8be157a519a44d2bb31276f58b3",
            0x69582b7b,
            0x5119f71,
            0x7b41ebf,
        ),
        (
            24149366,
            "0xdf0a78d741803286b4e70297c8744a0975a49e57929899b74d4cc1e02fb86c22",
            0x69582b87,
            0x5387e25,
            0x7b6c969,
        ),
        (
            24149367,
            "0x914dd799f9e86c2d653873fbd91fb2e358cc4181669645e311904a9c9bdfad87",
            0x69582b93,
            0x560df2e,
            0x7b77413,
        ),
        (
            24149368,
            "0x81d983e737e55ff824ec111b8206e11bb539f23402951e8e307605f1e6b6fb6e",
            0x69582b9f,
            0x522d59c,
            0x7bf7413,
        ),
        (
            24149369,
            "0xb92e0d84f79fb0e5e51d84121ff06391669f27f58c81e0af79deb577c0c298e5",
            0x69582bab,
            0x4e712a3,
            0x7c57413,
        ),
        (
            24149370,
            "0x4bc4d0197501b43b56f6a5eacc8250b0171ecc68391912ff89d07448f2d9b09b",
            0x69582bb7,
            0x4c57d14,
            0x7b57413,
        ),
        (
            24149371,
            "0x121df33b28185737da932a0f3c4ee8f4c665caf6f9d41df99f69f7b640ce89e3",
            0x69582bc3,
            0x4a60dc1,
            0x7ad7413,
        ),
        (
            24149372,
            "0x71ac6f9b2181e1fa6949d650d0c58f732bbc042cee5affd1306b1e52198e3bba",
            0x69582bcf,
            0x4a58936,
            0x79b7413,
        ),
        (
            24149373,
            "0x6c990ec218bc5e59cd67016ae84f8e35b422f02556f337ec7bed3c0f920ecea1",
            0x69582bdb,
            0x47e83bc,
            0x79f7413,
        ),
        (
            24149374,
            "0x211ac89f9b728057172585f7f882cb7b6db2bcc02c7a410e4f77e9fdc41808f5",
            0x69582be7,
            0x4967561,
            0x7a4c968,
        ),
        (
            24149375,
            "0x1bc6e64f58950863bf37f8888d94d300befdd2a2447127f0104fb26ae5c4834c",
            0x69582bf3,
            0x5280193,
            0x7a61ebd,
        ),
        (
            24149376,
            "0x5e7dc3eae77f0a551a95fdc54561feb6ca623437f65ea14c4377025d4c196d5a",
            0x69582bff,
            0x494dfd6,
            0x7a61ebd,
        ),
        (
            24149377,
            "0xdbc98a74196d4896a24d91ddf8b228cb272466449d34edf4997dd38478f43c46",
            0x69582c0b,
            0x51e706a,
            0x7ac1ebd,
        ),
        (
            24149378,
            "0xc42c59a9bc84bd6af53bfb15d5d64dc994c5dc4cc734e52f674304fd11efe155",
            0x69582c17,
            0x52a0d9e,
            0x7acc967,
        ),
        (
            24149379,
            "0xa268a7e8b7a4214add715f61ac0e5804028e58a641b308739bce5e420d1eb333",
            0x69582c23,
            0x5231586,
            0x7b0c967,
        ),
        (
            24149380,
            "0xf9969d3eeb6df8c5c18658821d1954fbbf5a054513cd38a8e340a93c6692a102",
            0x69582c2f,
            0x4a899c1,
            0x7b21ebc,
        ),
        (
            24149381,
            "0x21b447790661cca6addca9bca2ef0f30436c8768a8e36dbe59d4c63f7212396e",
            0x69582c3b,
            0x51992a2,
            0x7a01ebc,
        ),
        (
            24149382,
            "0xb1920c67f922b3c69913d4e3d65fac2e7cc74f93f37e9e2117970ed80a3fa09d",
            0x69582c47,
            0x558ac4a,
            0x7a41ebc,
        ),
        (
            24149383,
            "0x48818e8cf0211135c9533efa30e392ff8929bec5a4ab158687e82c8c4ec168b6",
            0x69582c53,
            0x4c943bb,
            0x7a41ebc,
        ),
        (
            24149384,
            "0xd757bee7dfbf5e12b1511463c51fbca99c1404aaeb824b9f60ef61d50b22865d",
            0x69582c5f,
            0x45b46d9,
            0x7ae1ebc,
        ),
        (
            24149385,
            "0xcebc9a62b9f9ac336de4b874b3ba5420b5dee2400e022d469636291edfd63f4c",
            0x69582c6b,
            0x4936c03,
            0x79e1ebc,
        ),
        (
            24149386,
            "0x98270cc47c9ec388bdfc564f7cc2b96af7399814d9a0db73759bb1592aab4afb",
            0x69582c77,
            0x4a64114,
            0x7a37411,
        ),
    ];

    for (block_num, hash_str, timestamp, basefee, excess_blob_gas) in real_l1_blocks {
        let hash: B256 = hash_str.parse().expect("valid hash");
        // EIP-4844: blob_basefee = f(excess_blob_gas)
        let blob_basefee = calc_blob_gasprice(excess_blob_gas);

        deriver.register_epoch(
            block_num,
            EpochInfo {
                l1_ref: L1BlockRef { number: block_num, hash, timestamp },
                basefee: U256::from(basefee),
                blob_basefee: U256::from(blob_basefee),
            },
        );
    }

    // Step 4: Process channels through Deriver
    let mut total_l2_blocks = 0;
    let mut total_l2_txs = 0;
    let mut all_blocks: Vec<L2Block> = Vec::new();

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

        all_blocks.extend(result.blocks);
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
        starting_l2_block + total_l2_blocks,
        "block number should advance by number of blocks built"
    );

    // Write blocks to JSON file for inspection
    let output_path = std::env::var("CARGO_MANIFEST_DIR")
        .map(|dir| format!("{}/l2_blocks.json", dir))
        .unwrap_or_else(|_| "l2_blocks.json".to_string());

    let mut file = File::create(&output_path)?;
    let json = serde_json::to_string_pretty(&all_blocks)?;
    file.write_all(json.as_bytes())?;

    tracing::info!(path = %output_path, "wrote L2 blocks to JSON file");

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
