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
use tracing;

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

/// EIP-2718 transaction type identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TxType {
    /// Legacy transaction (pre-EIP-2718)
    Legacy = 0x00,
    /// EIP-2930: Access list transaction
    Eip2930 = 0x01,
    /// EIP-1559: Dynamic fee transaction
    Eip1559 = 0x02,
    /// EIP-4844: Blob transaction (not used in L2)
    Eip4844 = 0x03,
    /// EIP-7702: Set code transaction
    Eip7702 = 0x04,
}

impl TxType {
    /// Try to parse a transaction type from a byte.
    /// Returns None for legacy transactions (which start with RLP list prefix >= 0xc0)
    pub fn from_first_byte(byte: u8) -> Option<Self> {
        match byte {
            0x01 => Some(TxType::Eip2930),
            0x02 => Some(TxType::Eip1559),
            0x03 => Some(TxType::Eip4844),
            0x04 => Some(TxType::Eip7702),
            // >= 0xc0 is RLP list prefix, indicating legacy tx
            b if b >= 0xc0 => None,
            _ => None,
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
    pub parent_hash: [u8; 32],
    pub epoch_num: u64,
    pub epoch_hash: [u8; 32],
    pub timestamp: u64,
    pub transactions: Vec<Bytes>,
}

/// A span batch contains transactions for multiple L2 blocks
#[derive(Debug, Clone)]
pub struct SpanBatch {
    pub rel_timestamp: u64,
    pub l1_origin_num: u64,
    pub parent_check: [u8; 20],
    pub l1_origin_check: [u8; 20],
    pub blocks: Vec<SpanBatchElement>,
}

/// A single block within a span batch
#[derive(Debug, Clone)]
pub struct SpanBatchElement {
    pub epoch_num: u64,
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

        tracing::debug!(
            block_count = block_count,
            total_tx_count = total_tx_count,
            "span batch: decoded block counts"
        );

        // --- SpanBatchTransactions (columnar format) ---
        let txs = SpanBatchTransactions::decode(cursor, total_tx_count)?;

        // Reconstruct full transactions from columnar data
        // TODO: Make chain_id configurable via rollup config
        const UNICHAIN_CHAIN_ID: u64 = 130;
        let reconstructed_txs = txs.reconstruct_transactions(UNICHAIN_CHAIN_ID);

        // Build blocks from decoded data
        // Per OP spec: l1_origin_num is the L1 origin of the LAST block in the span.
        // origin_bits indicate when the L1 origin changes (1 = changed from previous block).
        // We need to calculate the FIRST block's epoch by counting backwards.
        let origin_changes: u64 =
            (0..block_count).filter(|&i| i > 0 && get_bit(origin_bits, i)).count() as u64;
        let first_block_origin = l1_origin_num - origin_changes;

        let mut blocks = Vec::with_capacity(block_count);
        let mut tx_idx = 0usize;
        let mut current_l1_origin = first_block_origin;

        for (block_idx, &tx_count) in block_tx_counts.iter().enumerate() {
            // Check origin_bits for L1 origin change (increment epoch when bit is set)
            if get_bit(origin_bits, block_idx) && block_idx > 0 {
                current_l1_origin += 1;
            }

            let mut transactions = Vec::with_capacity(tx_count);
            for _ in 0..tx_count {
                if tx_idx < reconstructed_txs.len() {
                    transactions.push(reconstructed_txs[tx_idx].clone());
                }
                tx_idx += 1;
            }

            blocks.push(SpanBatchElement {
                epoch_num: current_l1_origin,
                timestamp: 0, // Computed by Deriver using rel_timestamp + block_idx
                transactions,
            });
        }

        Ok(Self { rel_timestamp, l1_origin_num, parent_check, l1_origin_check, blocks })
    }
}

/// Span batch transactions in columnar format
struct SpanBatchTransactions {
    contract_creation_bits: Vec<bool>,
    y_parity_bits: Vec<bool>,
    signatures: Vec<([u8; 32], [u8; 32])>, // (r, s)
    to_addresses: Vec<Address>,
    tx_data: Vec<Bytes>, // tx_type || rlp([type-specific fields])
    nonces: Vec<u64>,
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

        // 5. tx_data: Concatenated transaction payloads
        // Per OP spec, tx_datas contains RLP-encoded transaction fields:
        // - Legacy: rlp_encode(value, gasPrice, data) - ONE RLP list
        // - Typed: type_byte ++ rlp_encode(fields...)
        let mut tx_data = Vec::with_capacity(total_tx_count);
        for i in 0..total_tx_count {
            if cursor.is_empty() {
                // Out of tx_data - fill remaining with empty
                for _ in i..total_tx_count {
                    tx_data.push(Bytes::new());
                }
                break;
            }

            let start = cursor;
            let first_byte = cursor[0];

            // Determine tx type and skip the appropriate RLP structure
            let valid = if let Some(_tx_type) = TxType::from_first_byte(first_byte) {
                // Typed tx: type byte + ONE RLP list
                cursor = &cursor[1..]; // consume type byte
                skip_rlp_element(cursor)
                    .map(|rest| cursor = rest)
                    .is_some()
            } else if first_byte >= 0xc0 {
                // Legacy tx: starts with RLP list prefix (0xc0-0xff)
                skip_rlp_element(cursor)
                    .map(|rest| cursor = rest)
                    .is_some()
            } else {
                // Unexpected byte - not a valid tx_data start
                false
            };

            if valid {
                let consumed = start.len() - cursor.len();
                tx_data.push(Bytes::copy_from_slice(&start[..consumed]));
            } else {
                // Parsing failed - we've likely reached end of tx_datas section.
                // Fill remaining with empty and stop parsing.
                for _ in i..total_tx_count {
                    tx_data.push(Bytes::new());
                }
                break;
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

    /// Reconstruct full RLP-encoded transactions from columnar data.
    ///
    /// Each transaction is reconstructed by combining:
    /// - tx_type and type-specific fields from tx_data
    /// - nonce from nonces
    /// - gas_limit from gas_limits
    /// - to address from to_addresses (if not contract creation)
    /// - signature from signatures + y_parity_bits
    fn reconstruct_transactions(&self, chain_id: u64) -> Vec<Bytes> {
        let mut result = Vec::with_capacity(self.tx_data.len());
        let mut to_idx = 0usize;

        for i in 0..self.tx_data.len() {
            let tx_bytes = &self.tx_data[i];
            if tx_bytes.is_empty() {
                result.push(Bytes::new());
                if !self.contract_creation_bits.get(i).copied().unwrap_or(false) {
                    to_idx += 1; // still consume to_address slot
                }
                continue;
            }

            let first_byte = tx_bytes[0];

            // Determine tx type and fields
            let (tx_type, tx_fields) = match TxType::from_first_byte(first_byte) {
                Some(t) => (t, &tx_bytes[1..]),       // Typed tx: skip type byte
                None => (TxType::Legacy, tx_bytes.as_ref()), // Legacy: no type prefix
            };

            let nonce = self.nonces.get(i).copied().unwrap_or(0);
            let gas_limit = self.gas_limits.get(i).copied().unwrap_or(0);
            let is_create = self.contract_creation_bits.get(i).copied().unwrap_or(false);
            let y_parity = self.y_parity_bits.get(i).copied().unwrap_or(false);
            let (sig_r, sig_s) = self.signatures.get(i).copied().unwrap_or(([0u8; 32], [0u8; 32]));

            let to_addr = if is_create {
                None
            } else {
                let addr = self.to_addresses.get(to_idx).copied();
                to_idx += 1;
                addr
            };

            match reconstruct_tx(
                tx_type, tx_fields, chain_id, nonce, gas_limit, to_addr, y_parity, &sig_r, &sig_s,
            ) {
                Some(tx) => result.push(tx),
                None => {
                    tracing::debug!(tx_idx = i, tx_type = ?tx_type, "failed to reconstruct tx");
                    result.push(Bytes::new());
                }
            }
        }

        result
    }
}

/// Get a bit from a span batch bitlist.
/// Per OP Stack spec: "standard bitlists are encoded as big-endian integers,
/// left-padded with zeroes to the next multiple of 8 bits."
/// Go's big.Int.Bit(i) returns bit i counting from LSB (rightmost).
/// So bit 0 is the LSB (rightmost bit of the last byte).
fn get_bit(bits: &[u8], index: usize) -> bool {
    let total_bits = bits.len() * 8;
    let bit_from_msb = total_bits - 1 - index; // convert LSB index to MSB position
    let byte_idx = bit_from_msb / 8;
    let bit_in_byte = 7 - (bit_from_msb % 8);
    (bits[byte_idx] >> bit_in_byte) & 1 != 0
}

/// Skip over a single RLP element (string or list) and return the remaining data.
/// Returns None if the data is malformed or too short.
fn skip_rlp_element(data: &[u8]) -> Option<&[u8]> {
    if data.is_empty() {
        return None;
    }

    let first = data[0];

    // Single byte value (0x00-0x7f)
    if first < 0x80 {
        return Some(&data[1..]);
    }

    // Short string (0x80-0xb7): length is first_byte - 0x80
    if first <= 0xb7 {
        let len = (first - 0x80) as usize;
        if data.len() < 1 + len {
            return None;
        }
        return Some(&data[1 + len..]);
    }

    // Long string (0xb8-0xbf): next (first_byte - 0xb7) bytes are the length
    if first <= 0xbf {
        let len_of_len = (first - 0xb7) as usize;
        if data.len() < 1 + len_of_len {
            return None;
        }
        let mut len = 0usize;
        for i in 0..len_of_len {
            len = (len << 8) | (data[1 + i] as usize);
        }
        if data.len() < 1 + len_of_len + len {
            return None;
        }
        return Some(&data[1 + len_of_len + len..]);
    }

    // Short list (0xc0-0xf7): length is first_byte - 0xc0
    if first <= 0xf7 {
        let len = (first - 0xc0) as usize;
        if data.len() < 1 + len {
            return None;
        }
        return Some(&data[1 + len..]);
    }

    // Long list (0xf8-0xff): next (first_byte - 0xf7) bytes are the length
    let len_of_len = (first - 0xf7) as usize;
    if data.len() < 1 + len_of_len {
        return None;
    }
    let mut len = 0usize;
    for i in 0..len_of_len {
        len = (len << 8) | (data[1 + i] as usize);
    }
    if data.len() < 1 + len_of_len + len {
        return None;
    }
    Some(&data[1 + len_of_len + len..])
}

/// Reconstruct a full RLP-encoded transaction from span batch components.
///
/// tx_fields contains the type-specific RLP elements (value, fees, data, access_list)
/// that were extracted during span batch encoding.
fn reconstruct_tx(
    tx_type: TxType,
    tx_fields: &[u8],
    chain_id: u64,
    nonce: u64,
    gas_limit: u64,
    to: Option<Address>,
    y_parity: bool,
    sig_r: &[u8; 32],
    sig_s: &[u8; 32],
) -> Option<Bytes> {
    // Parse the type-specific fields from tx_fields
    let cursor = tx_fields;

    match tx_type {
        TxType::Legacy => {
            // Legacy: tx_fields = rlp([value, gasPrice, data])
            // Note: value comes FIRST in span batch format
            let inner = unwrap_rlp_list(cursor)?;
            let (value, rest) = decode_rlp_bytes(inner)?;
            let (gas_price, rest) = decode_rlp_bytes(rest)?;
            let (data, _) = decode_rlp_bytes(rest)?;

            // v = chain_id * 2 + 35 + y_parity (EIP-155)
            let v = chain_id * 2 + 35 + if y_parity { 1 } else { 0 };

            // Build: rlp([nonce, gas_price, gas_limit, to, value, data, v, r, s])
            let mut out = Vec::with_capacity(256);
            encode_legacy_tx(
                &mut out, nonce, &gas_price, gas_limit, to, &value, &data, v, sig_r, sig_s,
            );
            Some(Bytes::from(out))
        }
        TxType::Eip1559 => {
            // EIP-1559: tx_fields = rlp([value, max_priority_fee, max_fee, data, access_list])
            // Note: value comes FIRST in span batch format (different from standard tx encoding)
            let inner = unwrap_rlp_list(cursor)?;
            let (value, rest) = decode_rlp_bytes(inner)?;
            let (max_priority_fee, rest) = decode_rlp_bytes(rest)?;
            let (max_fee, rest) = decode_rlp_bytes(rest)?;
            let (data, rest) = decode_rlp_bytes(rest)?;
            let (access_list, _) = decode_rlp_bytes(rest)?;

            // Build: 0x02 || rlp([chain_id, nonce, max_priority_fee, max_fee, gas_limit, to, value,
            // data, access_list, y_parity, r, s])
            let mut out = Vec::with_capacity(512);
            out.push(TxType::Eip1559 as u8);
            encode_eip1559_tx(
                &mut out,
                chain_id,
                nonce,
                &max_priority_fee,
                &max_fee,
                gas_limit,
                to,
                &value,
                &data,
                &access_list,
                y_parity,
                sig_r,
                sig_s,
            );
            Some(Bytes::from(out))
        }
        TxType::Eip2930 => {
            // EIP-2930: tx_fields = rlp([value, gas_price, data, access_list])
            // Note: value comes FIRST in span batch format (different from standard tx encoding)
            let inner = unwrap_rlp_list(cursor)?;
            let (value, rest) = decode_rlp_bytes(inner)?;
            let (gas_price, rest) = decode_rlp_bytes(rest)?;
            let (data, rest) = decode_rlp_bytes(rest)?;
            let (access_list, _) = decode_rlp_bytes(rest)?;

            let mut out = Vec::with_capacity(512);
            out.push(TxType::Eip2930 as u8);
            encode_eip2930_tx(
                &mut out,
                chain_id,
                nonce,
                &gas_price,
                gas_limit,
                to,
                &value,
                &data,
                &access_list,
                y_parity,
                sig_r,
                sig_s,
            );
            Some(Bytes::from(out))
        }
        TxType::Eip7702 => {
            // EIP-7702 (Set Code Transaction)
            // tx_fields = rlp([value, max_priority_fee, max_fee, data, access_list, authorization_list])
            let inner = unwrap_rlp_list(cursor)?;
            let (value, rest) = decode_rlp_bytes(inner)?;
            let (max_priority_fee, rest) = decode_rlp_bytes(rest)?;
            let (max_fee, rest) = decode_rlp_bytes(rest)?;
            let (data, rest) = decode_rlp_bytes(rest)?;
            let (access_list, rest) = decode_rlp_bytes(rest)?;
            let (authorization_list, _) = decode_rlp_bytes(rest)?;

            let mut out = Vec::with_capacity(1024);
            out.push(TxType::Eip7702 as u8);
            encode_eip7702_tx(
                &mut out,
                chain_id,
                nonce,
                &max_priority_fee,
                &max_fee,
                gas_limit,
                to,
                &value,
                &data,
                &access_list,
                &authorization_list,
                y_parity,
                sig_r,
                sig_s,
            );
            Some(Bytes::from(out))
        }
        TxType::Eip4844 => {
            // EIP-4844 blob transactions are not used in L2
            // Because blob transactions only exist in L1
            tracing::debug!("EIP-4844 blob tx not supported in L2");
            None
        }
    }
}

/// Unwrap an RLP list and return a slice to its contents.
/// For span batch typed txs, the fields are wrapped in a list.
fn unwrap_rlp_list(data: &[u8]) -> Option<&[u8]> {
    if data.is_empty() {
        return None;
    }

    let first = data[0];

    // Short list (0xc0-0xf7): length is first - 0xc0
    if first >= 0xc0 && first <= 0xf7 {
        let len = (first - 0xc0) as usize;
        if data.len() < 1 + len {
            return None;
        }
        return Some(&data[1..1 + len]);
    }

    // Long list (0xf8-0xff): next (first - 0xf7) bytes are the length
    if first >= 0xf8 {
        let len_of_len = (first - 0xf7) as usize;
        if data.len() < 1 + len_of_len {
            return None;
        }
        let mut len = 0usize;
        for i in 0..len_of_len {
            len = (len << 8) | (data[1 + i] as usize);
        }
        if data.len() < 1 + len_of_len + len {
            return None;
        }
        return Some(&data[1 + len_of_len..1 + len_of_len + len]);
    }

    // Not a list
    None
}

/// Decode a single RLP element and return (raw_bytes_including_header, remaining)
fn decode_rlp_bytes(data: &[u8]) -> Option<(&[u8], &[u8])> {
    if data.is_empty() {
        return Some((&[], data));
    }

    let first = data[0];

    // Single byte value (0x00-0x7f)
    if first < 0x80 {
        return Some((&data[..1], &data[1..]));
    }

    // Short string (0x80-0xb7): length is first - 0x80
    if first <= 0xb7 {
        let len = (first - 0x80) as usize;
        let total = 1 + len;
        if data.len() < total {
            return None;
        }
        return Some((&data[..total], &data[total..]));
    }

    // Long string (0xb8-0xbf): next (first - 0xb7) bytes are the length
    if first <= 0xbf {
        let len_of_len = (first - 0xb7) as usize;
        if data.len() < 1 + len_of_len {
            return None;
        }
        let mut len_bytes = [0u8; 8];
        len_bytes[8 - len_of_len..].copy_from_slice(&data[1..1 + len_of_len]);
        let len = u64::from_be_bytes(len_bytes) as usize;
        let total = 1 + len_of_len + len;
        if data.len() < total {
            return None;
        }
        return Some((&data[..total], &data[total..]));
    }

    // Short list (0xc0-0xf7): length is first - 0xc0
    if first <= 0xf7 {
        let len = (first - 0xc0) as usize;
        let total = 1 + len;
        if data.len() < total {
            return None;
        }
        return Some((&data[..total], &data[total..]));
    }

    // Long list (0xf8-0xff): next (first - 0xf7) bytes are the length
    let len_of_len = (first - 0xf7) as usize;
    if data.len() < 1 + len_of_len {
        return None;
    }
    let mut len_bytes = [0u8; 8];
    len_bytes[8 - len_of_len..].copy_from_slice(&data[1..1 + len_of_len]);
    let len = u64::from_be_bytes(len_bytes) as usize;
    let total = 1 + len_of_len + len;
    if data.len() < total {
        return None;
    }
    Some((&data[..total], &data[total..]))
}

fn to_rlp_len(to: Option<Address>) -> usize {
    match to {
        Some(_) => 21, // 0x94 + 20 bytes
        None => 1,     // 0x80 (empty string)
    }
}

fn encode_to(out: &mut Vec<u8>, to: Option<Address>) {
    match to {
        Some(addr) => {
            out.push(0x80 + 20); // string of 20 bytes
            out.extend_from_slice(addr.as_slice());
        }
        None => {
            out.push(0x80); // empty string
        }
    }
}

fn encode_u256_bytes(out: &mut Vec<u8>, value: &[u8; 32]) {
    // Strip leading zeros
    let first_nonzero = value.iter().position(|&b| b != 0).unwrap_or(32);
    let trimmed = &value[first_nonzero..];

    if trimmed.is_empty() {
        out.push(0x80); // empty = 0
    } else if trimmed.len() == 1 && trimmed[0] < 0x80 {
        out.push(trimmed[0]); // single byte
    } else {
        out.push(0x80 + trimmed.len() as u8);
        out.extend_from_slice(trimmed);
    }
}

fn encode_legacy_tx(
    out: &mut Vec<u8>,
    nonce: u64,
    gas_price: &[u8],
    gas_limit: u64,
    to: Option<Address>,
    value: &[u8],
    data: &[u8],
    v: u64,
    r: &[u8; 32],
    s: &[u8; 32],
) {
    use alloy_rlp::Encodable;

    // Calculate payload length
    let payload_len = nonce.length() +
        gas_price.len() +
        gas_limit.length() +
        to_rlp_len(to) +
        value.len() +
        data.len() +
        v.length() +
        33 +
        33;

    // Encode list header
    alloy_rlp::Header { list: true, payload_length: payload_len }.encode(out);

    // Encode fields
    nonce.encode(out);
    out.extend_from_slice(gas_price);
    gas_limit.encode(out);
    encode_to(out, to);
    out.extend_from_slice(value);
    out.extend_from_slice(data);
    v.encode(out);
    encode_u256_bytes(out, r);
    encode_u256_bytes(out, s);
}

fn encode_eip1559_tx(
    out: &mut Vec<u8>,
    chain_id: u64,
    nonce: u64,
    max_priority_fee: &[u8],
    max_fee: &[u8],
    gas_limit: u64,
    to: Option<Address>,
    value: &[u8],
    data: &[u8],
    access_list: &[u8],
    y_parity: bool,
    r: &[u8; 32],
    s: &[u8; 32],
) {
    use alloy_rlp::Encodable;

    let y_parity_val: u8 = if y_parity { 1 } else { 0 };

    // Calculate payload length
    let payload_len = chain_id.length() +
        nonce.length() +
        max_priority_fee.len() +
        max_fee.len() +
        gas_limit.length() +
        to_rlp_len(to) +
        value.len() +
        data.len() +
        access_list.len() +
        y_parity_val.length() +
        33 +
        33;

    // Encode list header
    alloy_rlp::Header { list: true, payload_length: payload_len }.encode(out);

    // Encode fields
    chain_id.encode(out);
    nonce.encode(out);
    out.extend_from_slice(max_priority_fee);
    out.extend_from_slice(max_fee);
    gas_limit.encode(out);
    encode_to(out, to);
    out.extend_from_slice(value);
    out.extend_from_slice(data);
    out.extend_from_slice(access_list);
    y_parity_val.encode(out);
    encode_u256_bytes(out, r);
    encode_u256_bytes(out, s);
}

fn encode_eip2930_tx(
    out: &mut Vec<u8>,
    chain_id: u64,
    nonce: u64,
    gas_price: &[u8],
    gas_limit: u64,
    to: Option<Address>,
    value: &[u8],
    data: &[u8],
    access_list: &[u8],
    y_parity: bool,
    r: &[u8; 32],
    s: &[u8; 32],
) {
    use alloy_rlp::Encodable;

    let y_parity_val: u8 = if y_parity { 1 } else { 0 };

    let payload_len = chain_id.length() +
        nonce.length() +
        gas_price.len() +
        gas_limit.length() +
        to_rlp_len(to) +
        value.len() +
        data.len() +
        access_list.len() +
        y_parity_val.length() +
        33 +
        33;

    alloy_rlp::Header { list: true, payload_length: payload_len }.encode(out);

    chain_id.encode(out);
    nonce.encode(out);
    out.extend_from_slice(gas_price);
    gas_limit.encode(out);
    encode_to(out, to);
    out.extend_from_slice(value);
    out.extend_from_slice(data);
    out.extend_from_slice(access_list);
    y_parity_val.encode(out);
    encode_u256_bytes(out, r);
    encode_u256_bytes(out, s);
}

fn encode_eip7702_tx(
    out: &mut Vec<u8>,
    chain_id: u64,
    nonce: u64,
    max_priority_fee: &[u8],
    max_fee: &[u8],
    gas_limit: u64,
    to: Option<Address>,
    value: &[u8],
    data: &[u8],
    access_list: &[u8],
    authorization_list: &[u8],
    y_parity: bool,
    r: &[u8; 32],
    s: &[u8; 32],
) {
    use alloy_rlp::Encodable;

    let y_parity_val: u8 = if y_parity { 1 } else { 0 };

    // EIP-7702: [chain_id, nonce, max_priority_fee, max_fee, gas_limit, to, value,
    //            data, access_list, authorization_list, y_parity, r, s]
    let payload_len = chain_id.length()
        + nonce.length()
        + max_priority_fee.len()
        + max_fee.len()
        + gas_limit.length()
        + to_rlp_len(to)
        + value.len()
        + data.len()
        + access_list.len()
        + authorization_list.len()
        + y_parity_val.length()
        + 33
        + 33;

    alloy_rlp::Header { list: true, payload_length: payload_len }.encode(out);

    chain_id.encode(out);
    nonce.encode(out);
    out.extend_from_slice(max_priority_fee);
    out.extend_from_slice(max_fee);
    gas_limit.encode(out);
    encode_to(out, to);
    out.extend_from_slice(value);
    out.extend_from_slice(data);
    out.extend_from_slice(access_list);
    out.extend_from_slice(authorization_list);
    y_parity_val.encode(out);
    encode_u256_bytes(out, r);
    encode_u256_bytes(out, s);
}
