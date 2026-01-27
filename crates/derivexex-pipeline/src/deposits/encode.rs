//! RLP encoding for deposit transactions (type 0x7E).
//!
//! Deposit transactions use a custom RLP format:
//! `0x7E || rlp([source_hash, from, to, mint, value, gas, is_system_tx, data])`
//! Spec: <https://specs.optimism.io/protocol/deposits.html>

use alloy_primitives::{keccak256, B256};

use super::DepositedTransaction;

/// EIP-2718 transaction type for deposits, arbitrarily chosen by OP
pub const DEPOSIT_TX_TYPE: u8 = 0x7E;

/// Encode a deposit transaction into the provided buffer.
///
/// Format: `0x7E || rlp([source_hash, from, to, mint, value, gas, is_system_tx, data])`
pub fn encode_deposit_tx(tx: &DepositedTransaction, out: &mut Vec<u8>) {
    out.push(DEPOSIT_TX_TYPE);

    let mint_be = tx.mint.to_be_bytes::<32>();
    let value_be = tx.value.to_be_bytes::<32>();
    let gas_be = tx.gas_limit.to_be_bytes();

    let mint_trimmed = trim_leading_zeros(&mint_be);
    let value_trimmed = trim_leading_zeros(&value_be);
    let gas_trimmed = trim_leading_zeros(&gas_be);

    // `to` field: empty for contract creation, 20 bytes otherwise
    let to_bytes: &[u8] = match &tx.to {
        Some(addr) => addr.as_slice(),
        None => &[],
    };

    // is_system_tx: encoded as single byte 0x01 if true, empty if false
    let system_tx_bytes: &[u8] = if tx.is_system_tx { &[1] } else { &[] };

    // Calculate total payload length
    let payload_len = rlp_string_len(tx.source_hash.as_slice()) +
        rlp_string_len(tx.from.as_slice()) +
        rlp_string_len(to_bytes) +
        rlp_string_len(mint_trimmed) +
        rlp_string_len(value_trimmed) +
        rlp_string_len(gas_trimmed) +
        rlp_string_len(system_tx_bytes) +
        rlp_string_len(&tx.data);

    encode_list_header(out, payload_len);

    encode_string(out, tx.source_hash.as_slice());
    encode_string(out, tx.from.as_slice());
    encode_string(out, to_bytes);
    encode_string(out, mint_trimmed);
    encode_string(out, value_trimmed);
    encode_string(out, gas_trimmed);
    encode_string(out, system_tx_bytes);
    encode_string(out, &tx.data);
}

/// Compute the keccak256 hash of the encoded deposit transaction.
#[inline]
pub fn tx_hash(tx: &DepositedTransaction) -> B256 {
    let mut buf = Vec::with_capacity(256);
    encode_deposit_tx(tx, &mut buf);
    keccak256(&buf)
}

/// Trim leading zero bytes from a big-endian encoded integer.
/// Returns empty slice for zero values.
#[inline]
pub(crate) fn trim_leading_zeros(bytes: &[u8]) -> &[u8] {
    match bytes.iter().position(|&b| b != 0) {
        Some(idx) => &bytes[idx..],
        None => &[],
    }
}

/// Calculate the RLP-encoded length of a string/bytes.
#[inline]
const fn rlp_string_len(data: &[u8]) -> usize {
    let len = data.len();
    if len == 1 && data[0] < 0x80 {
        1
    } else if len < 56 {
        1 + len
    } else {
        1 + len_of_length(len) + len
    }
}

/// Calculate bytes needed to encode a length value.
#[inline]
const fn len_of_length(len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (usize::BITS as usize - len.leading_zeros() as usize + 7) / 8
    }
}

/// Encode an RLP list header.
#[inline]
fn encode_list_header(out: &mut Vec<u8>, payload_len: usize) {
    if payload_len < 56 {
        out.push(0xC0 + payload_len as u8);
    } else {
        let len_bytes = len_of_length(payload_len);
        out.push(0xF7 + len_bytes as u8);
        out.extend_from_slice(&payload_len.to_be_bytes()[8 - len_bytes..]);
    }
}

/// Encode an RLP string (bytes).
#[inline]
fn encode_string(out: &mut Vec<u8>, data: &[u8]) {
    let len = data.len();

    if len == 1 && data[0] < 0x80 {
        // Single byte < 0x80: encoded as itself
        out.push(data[0]);
    } else if len < 56 {
        // Short string: 0x80 + len, then data
        out.push(0x80 + len as u8);
        out.extend_from_slice(data);
    } else {
        // Long string: 0xB7 + len_of_len, then len, then data
        let len_bytes = len_of_length(len);
        out.push(0xB7 + len_bytes as u8);
        out.extend_from_slice(&len.to_be_bytes()[8 - len_bytes..]);
        out.extend_from_slice(data);
    }
}
