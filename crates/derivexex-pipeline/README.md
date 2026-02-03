# derivexex-pipeline

OP Stack derivation pipeline for Unichain. Decodes blobs, frames, channels, and batches into L2 blocks.

## Overview

The pipeline processes data through these stages:

1. **Blob decoding** - EIP-4844 field element encoding → raw bytes
2. **Frame parsing** - Raw bytes → channel frames with ordering metadata
3. **Channel assembly** - Frames → complete compressed channels
4. **Batch decoding** - Decompressed channels → SingleBatch or SpanBatch
5. **Block building** - Batches + deposits + L1 info → L2 blocks

## Quick Start

```rust
use derivexex_pipeline::{decode_blob_data, FrameDecoder, ChannelAssembler, Deriver};

// Decode blob → frames → channel → L2 blocks
let data = decode_blob_data(&blob);
let frames = FrameDecoder::decode_frames(&data, l1_block_number)?;

let mut assembler = ChannelAssembler::new();
for frame in frames {
    assembler.add_frame(frame);
}

let channels = assembler.take_complete();
for channel in channels {
    let result = deriver.process_channel(&channel)?;
    for block in result.blocks {
        println!("L2 block {} with {} txs", block.number, block.tx_count());
    }
}
```

Or use the high-level helper:

```rust
use derivexex_pipeline::decode_blobs_to_transactions;

let transactions = decode_blobs_to_transactions(&blobs)?;
```

## Components

### Blob Decoding

EIP-4844 blobs use field element encoding where each 32-byte chunk has its high 2 bits zeroed (BLS12-381 constraint). We decode this back to raw bytes:

```rust
use derivexex_pipeline::{decode_blob_data, decode_blob_data_into, max_blob_data_size};

// Allocating version
let data: Vec<u8> = decode_blob_data(&blob);

// Zero-copy into existing buffer (for hot paths)
let mut buf = vec![0u8; max_blob_data_size()];
let len = decode_blob_data_into(&blob, &mut buf);
```

The encoding reassembles 4×6-bit chunks from field element high bytes into 3 bytes of output per round.

### Frame Decoding

Frames are chunks of channel data with ordering metadata:

```rust
use derivexex_pipeline::{FrameDecoder, ChannelFrame};

let frames: Vec<ChannelFrame> = FrameDecoder::decode_frames(&data, l1_origin)?;

// Frame structure
struct ChannelFrame {
    channel_id: [u8; 16],  // Which channel this belongs to
    frame_number: u16,      // Order within channel
    frame_data: Vec<u8>,    // Payload
    is_last: bool,          // Final frame marker
    l1_origin: u64,         // L1 block number
}
```

Frame format: `channel_id (16) || frame_number (2) || length (4) || data || is_last (1)`

### Channel Assembly

Channels span multiple frames (potentially across blobs). The assembler handles out-of-order frames:

```rust
use derivexex_pipeline::ChannelAssembler;

let mut assembler = ChannelAssembler::new();

// Add frames from multiple blobs
for frame in frames_from_blob_1 {
    assembler.add_frame(frame);
}
for frame in frames_from_blob_2 {
    assembler.add_frame(frame);
}

// Get complete channels (all frames received)
let complete: Vec<Channel> = assembler.take_complete();

// Decompress channel data (zlib)
let data = channel.decompress()?;
```

### Batch Decoding

Two batch formats:

**SingleBatch** (type 0): One L2 block
```rust
struct SingleBatch {
    parent_hash: [u8; 32],
    epoch_num: u64,        // L1 block number
    epoch_hash: [u8; 32],  // L1 block hash
    timestamp: u64,        // L2 block timestamp
    transactions: Vec<Bytes>,
}
```

**SpanBatch** (type 1): Multiple L2 blocks, compressed
```rust
struct SpanBatch {
    rel_timestamp: u64,       // Relative to L2 genesis
    l1_origin_num: u64,       // Starting L1 block
    parent_check: [u8; 20],   // Truncated parent hash
    l1_origin_check: [u8; 20],// Truncated L1 hash
    blocks: Vec<SpanBatchElement>,
}
```

SpanBatch uses bit-packed encoding for transaction types and signature recovery bits.

### Deposit Parsing

Parse `TransactionDeposited` events from OptimismPortal:

```rust
use derivexex_pipeline::{DepositedTransaction, TRANSACTION_DEPOSITED_TOPIC};

// From log data
let deposit = DepositedTransaction::from_log(
    &log.data,
    &log.topics,
    l1_block_hash,
    log_index,
)?;

// Encode as L2 transaction (type 0x7E)
let tx_bytes = deposit.to_bytes();
let tx_hash = deposit.tx_hash();
```

Handles address aliasing for contract senders automatically.

### L1 Info Transactions

Every L2 block starts with L1 attributes. Supports Ecotone and Isthmus hardforks:

```rust
use derivexex_pipeline::{L1BlockInfo, Hardfork};

let l1_info = L1BlockInfo::new(
    l1_number,
    l1_timestamp,
    basefee,
    l1_hash,
    sequence_number,
    batcher_addr,
    blob_basefee,
    base_fee_scalar,
    blob_base_fee_scalar,
);

// Encode for Ecotone (164 bytes packed)
let calldata = l1_info.to_calldata(Hardfork::Ecotone);

// Or Isthmus (180 bytes, adds operator fees)
// Note that Unichain doesn't use Isthmus as of Feb 2, 2026, but this was added since it has breaking changes specifically to the L1 info encoding
let calldata = l1_info
    .with_operator_fees(scalar, constant)
    .to_calldata(Hardfork::Isthmus);

// Convert to deposit transaction
let deposit_tx = l1_info.to_deposit_tx(Hardfork::Ecotone);
```

### High-Level Deriver

Orchestrates the full pipeline:

```rust
use derivexex_pipeline::{Deriver, DeriverConfig, EpochInfo};

let config = DeriverConfig::builder()
    .hardfork(Hardfork::Ecotone)
    .batcher_addr(BATCHER)
    .l2_genesis_time(L2_GENESIS_TIME)
    .l2_block_time(1)  // 1 second blocks
    .build();

let mut deriver = Deriver::new(config);

// Register L1 block info (epochs)
deriver.register_epoch(l1_block_number, EpochInfo {
    l1_ref: L1BlockRef { number, hash, timestamp },
    basefee,
    blob_basefee,
});

// Add deposits for this epoch
deriver.add_deposit(l1_block_number, deposit);

// Process channel → L2 blocks
let result = deriver.process_channel(&channel)?;
println!("Built {} blocks with {} txs", result.blocks_built, result.txs_count);
```

#### Reorg Handling

```rust
// On L1 reorg, clear state from invalid blocks
deriver.clear_epochs_from(first_invalid_l1_block);
```

## Module Structure

- `lib.rs` - Blob decoding, public API
- `frame.rs` - Frame parsing
- `channel.rs` - Channel assembly
- `batch.rs` - SingleBatch + SpanBatch decoding
- `block.rs` - L2Block building
- `derive.rs` - High-level Deriver
- `deposits/` - DepositedTransaction parsing and type 0x7E encoding
- `l1_info/` - L1BlockInfo and Ecotone/Isthmus packed encoding
- `config.rs` - Unichain constants

## Unichain Specifics

See `config.rs` for all addresses and constants.

## References

- [OP Stack Derivation Spec](https://specs.optimism.io/protocol/derivation.html)
- [Frame Format](https://specs.optimism.io/protocol/derivation.html#frame-format)
- [Batch Format](https://specs.optimism.io/protocol/derivation.html#batch-format)
- [Deposits Spec](https://specs.optimism.io/protocol/deposits.html)
- [EIP-4844 Blob Encoding](https://github.com/ethereum/EIPs/blob/master/EIPS/eip-4844.md)
