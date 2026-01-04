//! Batch decoding for OP Stack rlp enconded batches.
//!
//! Ported from kona-protocol (https://github.com/op-rs/kona)
//! Supports both SingleBatch (type 0) and SpanBatch (type 1, introduced in Delta network upgrade).
//!
//! Why didn't I just use kona crates?
//! 1 - Kona's BatchReader required a RollupConfig to be instantiated with a lot of parameters that
//! were not really needed for this simple project
//! 2 - I also faced some dependency issues between op-alloy-protocol / kona-derive / reth
//! 3 - There are some repeated dependencies between reth / kona that also would add some extra time
//! to fix 4 - In the end, writing the code while researching and understand is a better learning
//! experience and possibly faster

use alloy_primitives::{Address, Bytes};
use alloy_rlp::Decodable;
use reth_tracing::tracing;

/// Batch type identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum BatchType {
    Single = 0,
    Span = 1,
}

impl From<u8> for BatchType {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::Single,
            1 => Self::Span,
            _ => Self::Single, // Default to single for unknown types
        }
    }
}

/// A decoded batch - either Single or Span
#[derive(Debug, Clone)]
pub enum Batch {
    Single(SingleBatch),
    Span(SpanBatch),
}

/// A single batch contains transactions for one L2 block
#[derive(Debug, Clone)]
pub struct SingleBatch {
    #[allow(dead_code)]
    pub parent_hash: [u8; 32],
    pub epoch_num: u64,
    #[allow(dead_code)]
    pub epoch_hash: [u8; 32],
    pub timestamp: u64,
    pub transactions: Vec<Bytes>,
}

/// A span batch contains transactions for multiple L2 blocks
#[derive(Debug, Clone)]
pub struct SpanBatch {
    #[allow(dead_code)]
    pub rel_timestamp: u64,
    pub l1_origin_num: u64,
    #[allow(dead_code)]
    pub parent_check: [u8; 20],
    #[allow(dead_code)]
    pub l1_origin_check: [u8; 20],
    pub blocks: Vec<SpanBatchElement>,
}

/// A single block within a span batch
#[derive(Debug, Clone)]
pub struct SpanBatchElement {
    #[allow(dead_code)]
    pub epoch_num: u64,
    #[allow(dead_code)]
    pub timestamp: u64,
    pub transactions: Vec<Bytes>,
}

/// Decoded transaction from span batch (columnar format)
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SpanBatchTx {
    pub tx_type: u8,
    pub to: Option<Address>,
    pub nonce: u64,
    pub gas_limit: u64,
    pub data: Bytes,
    pub y_parity: bool,
    pub sig_r: [u8; 32],
    pub sig_s: [u8; 32],
}

#[derive(Debug, thiserror::Error)]
pub enum BatchError {
    #[error("Empty buffer")]
    EmptyBuffer,
    #[error("RLP decode error: {0}")]
    RlpError(String),
    #[error("Span batch error: {0}")]
    SpanError(String),
    #[error("Buffer too short")]
    BufferTooShort,
}

impl Batch {
    /// Decode a batch from raw bytes (after RLP outer decode)
    pub fn decode(data: &[u8]) -> Result<Self, BatchError> {
        if data.is_empty() {
            return Err(BatchError::EmptyBuffer);
        }

        let batch_type = BatchType::from(data[0]);

        match batch_type {
            BatchType::Single => {
                let single = SingleBatch::decode(&data[1..])?;
                Ok(Batch::Single(single))
            }
            BatchType::Span => {
                let span = SpanBatch::decode(&data[1..])?;
                Ok(Batch::Span(span))
            }
        }
    }
}

impl SingleBatch {
    /// Decode a single batch from RLP-encoded data
    pub fn decode(data: &[u8]) -> Result<Self, BatchError> {
        // SingleBatch is RLP: [parent_hash, epoch_num, epoch_hash, timestamp, [tx1, tx2, ...]]
        let mut cursor = data;

        // Decode as RLP list
        let header = alloy_rlp::Header::decode(&mut cursor)
            .map_err(|e| BatchError::RlpError(e.to_string()))?;

        if !header.list {
            return Err(BatchError::RlpError("Expected RLP list".to_string()));
        }

        // parent_hash (32 bytes)
        let parent_hash_bytes =
            Bytes::decode(&mut cursor).map_err(|e| BatchError::RlpError(e.to_string()))?;
        let mut parent_hash = [0u8; 32];
        if parent_hash_bytes.len() == 32 {
            parent_hash.copy_from_slice(&parent_hash_bytes);
        }

        // epoch_num (varint/u64)
        let epoch_num =
            u64::decode(&mut cursor).map_err(|e| BatchError::RlpError(e.to_string()))?;

        // epoch_hash (32 bytes)
        let epoch_hash_bytes =
            Bytes::decode(&mut cursor).map_err(|e| BatchError::RlpError(e.to_string()))?;
        let mut epoch_hash = [0u8; 32];
        if epoch_hash_bytes.len() == 32 {
            epoch_hash.copy_from_slice(&epoch_hash_bytes);
        }

        // timestamp
        let timestamp =
            u64::decode(&mut cursor).map_err(|e| BatchError::RlpError(e.to_string()))?;

        // transactions list
        let tx_header = alloy_rlp::Header::decode(&mut cursor)
            .map_err(|e| BatchError::RlpError(e.to_string()))?;

        let mut transactions = Vec::new();
        let tx_end = cursor.len().saturating_sub(tx_header.payload_length);

        while cursor.len() > tx_end {
            let tx = Bytes::decode(&mut cursor).map_err(|e| BatchError::RlpError(e.to_string()))?;
            transactions.push(tx);
        }

        Ok(Self { parent_hash, epoch_num, epoch_hash, timestamp, transactions })
    }
}

impl SpanBatch {
    /// Decode a span batch from raw bytes
    pub fn decode(data: &[u8]) -> Result<Self, BatchError> {
        let mut cursor = data;

        // --- SpanBatchPrefix ---
        let (rel_timestamp, rest) = unsigned_varint::decode::u64(cursor)
            .map_err(|_| BatchError::SpanError("Failed to decode rel_timestamp".to_string()))?;
        cursor = rest;

        let (l1_origin_num, rest) = unsigned_varint::decode::u64(cursor)
            .map_err(|_| BatchError::SpanError("Failed to decode l1_origin_num".to_string()))?;
        cursor = rest;

        if cursor.len() < 40 {
            return Err(BatchError::BufferTooShort);
        }
        let mut parent_check = [0u8; 20];
        let mut l1_origin_check = [0u8; 20];
        parent_check.copy_from_slice(&cursor[..20]);
        l1_origin_check.copy_from_slice(&cursor[20..40]);
        cursor = &cursor[40..];

        // --- SpanBatchPayload ---
        let (block_count, rest) = unsigned_varint::decode::u64(cursor)
            .map_err(|_| BatchError::SpanError("Failed to decode block_count".to_string()))?;
        cursor = rest;

        if block_count == 0 || block_count > 10_000_000 {
            return Err(BatchError::SpanError(format!("Invalid block_count: {}", block_count)));
        }
        let block_count = block_count as usize;

        // origin_bits (1 bit per block)
        let origin_bits_len = (block_count + 7) / 8;
        if cursor.len() < origin_bits_len {
            return Err(BatchError::BufferTooShort);
        }
        let origin_bits = &cursor[..origin_bits_len];
        cursor = &cursor[origin_bits_len..];

        // block_tx_counts (varint per block)
        let mut block_tx_counts = Vec::with_capacity(block_count);
        let mut total_tx_count = 0usize;
        for _ in 0..block_count {
            let (tx_count, rest) = unsigned_varint::decode::u64(cursor).map_err(|_| {
                BatchError::SpanError("Failed to decode block_tx_count".to_string())
            })?;
            block_tx_counts.push(tx_count as usize);
            total_tx_count += tx_count as usize;
            cursor = rest;
        }

        // --- SpanBatchTransactions (columnar format) ---
        let txs = SpanBatchTransactions::decode(cursor, total_tx_count)?;

        // Build blocks from decoded data
        let mut blocks = Vec::with_capacity(block_count);
        let mut tx_idx = 0usize;
        let mut current_l1_origin = l1_origin_num;

        for (block_idx, &tx_count) in block_tx_counts.iter().enumerate() {
            // Check origin_bits for L1 origin change
            if get_bit(origin_bits, block_idx) && block_idx > 0 {
                current_l1_origin -= 1;
            }

            let mut transactions = Vec::with_capacity(tx_count);
            for _ in 0..tx_count {
                if tx_idx < txs.tx_data.len() {
                    transactions.push(txs.tx_data[tx_idx].clone());
                }
                tx_idx += 1;
            }

            blocks.push(SpanBatchElement {
                epoch_num: current_l1_origin,
                timestamp: 0, // Would need genesis + rel_timestamp + block_idx * block_time
                transactions,
            });
        }

        Ok(Self { rel_timestamp, l1_origin_num, parent_check, l1_origin_check, blocks })
    }
}

/// Span batch transactions in columnar format
struct SpanBatchTransactions {
    #[allow(dead_code)]
    contract_creation_bits: Vec<bool>,
    #[allow(dead_code)]
    y_parity_bits: Vec<bool>,
    #[allow(dead_code)]
    signatures: Vec<([u8; 32], [u8; 32])>, // (r, s)
    #[allow(dead_code)]
    to_addresses: Vec<Address>,
    tx_data: Vec<Bytes>,
    #[allow(dead_code)]
    nonces: Vec<u64>,
    #[allow(dead_code)]
    gas_limits: Vec<u64>,
}

impl SpanBatchTransactions {
    fn decode(data: &[u8], total_tx_count: usize) -> Result<Self, BatchError> {
        let mut cursor = data;

        // 1. contract_creation_bits
        let cc_bits_len = (total_tx_count + 7) / 8;
        if cursor.len() < cc_bits_len {
            return Err(BatchError::BufferTooShort);
        }
        let cc_bits_raw = &cursor[..cc_bits_len];
        cursor = &cursor[cc_bits_len..];

        let mut contract_creation_bits = vec![false; total_tx_count];
        let mut contract_count = 0usize;
        for i in 0..total_tx_count {
            let is_create = get_bit(cc_bits_raw, i);
            contract_creation_bits[i] = is_create;
            if is_create {
                contract_count += 1;
            }
        }
        let regular_count = total_tx_count - contract_count;

        // 2. y_parity_bits
        let y_bits_len = (total_tx_count + 7) / 8;
        if cursor.len() < y_bits_len {
            return Err(BatchError::BufferTooShort);
        }
        let y_bits_raw = &cursor[..y_bits_len];
        cursor = &cursor[y_bits_len..];

        let mut y_parity_bits = vec![false; total_tx_count];
        for i in 0..total_tx_count {
            y_parity_bits[i] = get_bit(y_bits_raw, i);
        }

        // 3. signatures: 64 bytes (r + s) per tx
        let sigs_len = total_tx_count * 64;
        if cursor.len() < sigs_len {
            return Err(BatchError::BufferTooShort);
        }
        let sigs_raw = &cursor[..sigs_len];
        cursor = &cursor[sigs_len..];

        let mut signatures = Vec::with_capacity(total_tx_count);
        for i in 0..total_tx_count {
            let mut r = [0u8; 32];
            let mut s = [0u8; 32];
            r.copy_from_slice(&sigs_raw[i * 64..i * 64 + 32]);
            s.copy_from_slice(&sigs_raw[i * 64 + 32..i * 64 + 64]);
            signatures.push((r, s));
        }

        // 4. tx_tos: 20 bytes per non-contract-creation tx
        let tos_len = regular_count * 20;
        if cursor.len() < tos_len {
            return Err(BatchError::BufferTooShort);
        }
        let tos_raw = &cursor[..tos_len];
        cursor = &cursor[tos_len..];

        let mut to_addresses = Vec::with_capacity(regular_count);
        for i in 0..regular_count {
            to_addresses.push(Address::from_slice(&tos_raw[i * 20..(i + 1) * 20]));
        }

        // 5. tx_data: RLP-encoded per tx (tx_type byte + type-specific data)
        let mut tx_data = Vec::with_capacity(total_tx_count);
        for i in 0..total_tx_count {
            match decode_span_tx_data(cursor) {
                Some((data, rest)) => {
                    tx_data.push(data);
                    cursor = rest;
                }
                None => {
                    tracing::debug!(
                        tx_idx = i,
                        remaining = cursor.len(),
                        "stopping tx_data decode"
                    );
                    // Fill remaining with empty
                    for _ in i..total_tx_count {
                        tx_data.push(Bytes::new());
                    }
                    break;
                }
            }
        }

        // 6. tx_nonces: varint per tx
        let mut nonces = Vec::with_capacity(total_tx_count);
        for _ in 0..total_tx_count {
            match unsigned_varint::decode::u64(cursor) {
                Ok((nonce, rest)) => {
                    nonces.push(nonce);
                    cursor = rest;
                }
                Err(_) => {
                    // Fill remaining with 0
                    while nonces.len() < total_tx_count {
                        nonces.push(0);
                    }
                    break;
                }
            }
        }

        // 7. tx_gases: varint per tx
        let mut gas_limits = Vec::with_capacity(total_tx_count);
        for _ in 0..total_tx_count {
            match unsigned_varint::decode::u64(cursor) {
                Ok((gas, rest)) => {
                    gas_limits.push(gas);
                    cursor = rest;
                }
                Err(_) => {
                    while gas_limits.len() < total_tx_count {
                        gas_limits.push(0);
                    }
                    break;
                }
            }
        }

        Ok(Self {
            contract_creation_bits,
            y_parity_bits,
            signatures,
            to_addresses,
            tx_data,
            nonces,
            gas_limits,
        })
    }
}

/// Get a bit from a bitfield (little-endian bit order within each byte, per Kona)
#[inline]
fn get_bit(bits: &[u8], index: usize) -> bool {
    let byte_idx = index / 8;
    let bit_idx = index % 8;
    (bits[byte_idx] >> bit_idx) & 1 != 0
}

/// Decode SpanBatchTransactionData (tx_type + type-specific RLP)
fn decode_span_tx_data(data: &[u8]) -> Option<(Bytes, &[u8])> {
    if data.is_empty() {
        return None;
    }

    let tx_type = data[0];
    let mut cursor = &data[1..];
    let start_len = cursor.len();

    // Skip RLP elements based on tx_type
    match tx_type {
        0 => {
            // Legacy: just value
            cursor = skip_rlp_element(cursor)?;
        }
        1 => {
            // EIP2930: value + access_list
            cursor = skip_rlp_element(cursor)?; // value
            cursor = skip_rlp_element(cursor)?; // access_list
        }
        2 | 3 => {
            // EIP1559/EIP4844: value + max_priority_fee + max_fee + access_list
            cursor = skip_rlp_element(cursor)?; // value
            cursor = skip_rlp_element(cursor)?; // max_priority_fee
            cursor = skip_rlp_element(cursor)?; // max_fee
            cursor = skip_rlp_element(cursor)?; // access_list
        }
        _ => return None,
    }

    let consumed = 1 + (start_len - cursor.len());
    let tx_data = Bytes::copy_from_slice(&data[..consumed]);
    Some((tx_data, cursor))
}

/// Skip an RLP element and return the remaining slice
fn skip_rlp_element(data: &[u8]) -> Option<&[u8]> {
    if data.is_empty() {
        return None;
    }

    let first = data[0];

    // Single byte (0x00-0x7f)
    if first < 0x80 {
        return Some(&data[1..]);
    }

    // Short string (0x80-0xb7)
    if first <= 0xb7 {
        let len = (first - 0x80) as usize;
        let total = 1 + len;
        return data.get(total..).or(None);
    }

    // Long string (0xb8-0xbf)
    if first <= 0xbf {
        let len_of_len = (first - 0xb7) as usize;
        if data.len() < 1 + len_of_len {
            return None;
        }
        let mut len_bytes = [0u8; 8];
        len_bytes[8 - len_of_len..].copy_from_slice(&data[1..1 + len_of_len]);
        let len = u64::from_be_bytes(len_bytes) as usize;
        let total = 1 + len_of_len + len;
        return data.get(total..).or(None);
    }

    // Short list (0xc0-0xf7)
    if first <= 0xf7 {
        let len = (first - 0xc0) as usize;
        let total = 1 + len;
        return data.get(total..).or(None);
    }

    // Long list (0xf8-0xff)
    let len_of_len = (first - 0xf7) as usize;
    if data.len() < 1 + len_of_len {
        return None;
    }
    let mut len_bytes = [0u8; 8];
    len_bytes[8 - len_of_len..].copy_from_slice(&data[1..1 + len_of_len]);
    let len = u64::from_be_bytes(len_bytes) as usize;
    let total = 1 + len_of_len + len;
    data.get(total..).or(None)
}
