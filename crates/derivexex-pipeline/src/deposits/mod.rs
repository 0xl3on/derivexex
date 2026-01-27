//! Deposit transaction parsing and encoding for OP Stack L1→L2 deposits.
//!
//! In OP Stack, "deposit transactions" are any L1-initiated transactions that execute on L2.
//! This includes ETH bridges, token bridges, arbitrary L2 calls, and system deposits.

pub mod encode;
pub mod parse;

use alloy_primitives::{Address, Bytes, B256, U256};
use serde::Serialize;
use thiserror::Error;

pub use encode::DEPOSIT_TX_TYPE;
pub use parse::TRANSACTION_DEPOSITED_TOPIC;

#[derive(Debug, Error)]
pub enum DepositError {
    #[error("invalid opaque data length: expected at least 73 bytes, got {0}")]
    InvalidOpaqueDataLength(usize),
    #[error("unsupported deposit version: {0}")]
    UnsupportedVersion(U256),
    #[error("failed to decode log: {0}")]
    DecodeError(#[from] alloy_sol_types::Error),
}

/// A decoded L1→L2 deposit transaction.
///
/// Created by parsing `TransactionDeposited` events from the OptimismPortal contract.
/// Can be encoded to the L2 deposit transaction format (type 0x7E) for block reconstruction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DepositedTransaction {
    pub source_hash: B256,
    pub from: Address,
    pub to: Option<Address>,
    pub mint: U256,
    pub value: U256,
    pub gas_limit: u64,
    pub is_system_tx: bool,
    pub data: Bytes,
}

impl DepositedTransaction {
    /// Parse a deposit from a `TransactionDeposited` event log.
    ///
    /// # Arguments
    /// * `log_data` - The non-indexed event data (opaqueData)
    /// * `topics` - The log topics (event signature + indexed params)
    /// * `l1_block_hash` - The L1 block containing this event
    /// * `log_index` - The index of this log within the block
    #[inline]
    pub fn from_log(
        log_data: &[u8],
        topics: &[B256],
        l1_block_hash: B256,
        log_index: u64,
    ) -> Result<Self, DepositError> {
        parse::from_log(log_data, topics, l1_block_hash, log_index)
    }

    /// Encode this deposit as an L2 transaction (type 0x7E).
    ///
    /// Writes `0x7E || rlp([source_hash, from, to, mint, value, gas, is_system_tx, data])`
    /// into the provided buffer.
    #[inline]
    pub fn encode(&self, out: &mut Vec<u8>) {
        encode::encode_deposit_tx(self, out)
    }

    /// Encode this deposit and return as `Bytes`.
    ///
    /// Prefer [`encode`](Self::encode) with a reusable buffer when encoding multiple deposits.
    pub fn to_bytes(&self) -> Bytes {
        let mut buf = Vec::with_capacity(256);
        self.encode(&mut buf);
        Bytes::from(buf)
    }

    /// Compute the L2 transaction hash.
    #[inline]
    pub fn tx_hash(&self) -> B256 {
        encode::tx_hash(self)
    }
}
