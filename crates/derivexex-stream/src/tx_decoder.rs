//! Transaction decoding using op-alloy-consensus.

use alloy_consensus::Transaction;
use alloy_primitives::{Address, Bytes, B256, U256};
use alloy_rlp::Decodable;
use op_alloy_consensus::{OpTxEnvelope, OpTxType};
use serde::Serialize;

use derivexex_pipeline::DepositedTransaction;

use super::PipelineError;

/// A decoded transaction with L2 block context.
#[derive(Debug, Clone, Serialize)]
pub struct DecodedTransaction {
    /// Transaction hash.
    pub hash: B256,
    /// Sender address (recovered from signature for non-deposits).
    pub from: Address,
    /// Recipient address (None for contract creation).
    pub to: Option<Address>,
    /// Value transferred.
    pub value: U256,
    /// Input data.
    pub input: Bytes,
    /// Transaction type (0 = legacy, 1 = EIP-2930, 2 = EIP-1559, 3 = EIP-4844, 0x7E = deposit).
    pub tx_type: u8,
    /// Nonce.
    pub nonce: u64,
    /// Gas limit.
    pub gas_limit: u64,
    /// L2 block timestamp this transaction is included in.
    pub l2_block_timestamp: u64,
    /// L1 origin block number.
    pub l1_origin_number: u64,
    /// Whether this is a deposit transaction.
    pub is_deposit: bool,
}

/// Decode a transaction from raw bytes using op-alloy-consensus.
///
/// Handles all transaction types including deposits (0x7E).
pub fn decode_transaction(
    raw: &[u8],
    l2_block_timestamp: u64,
    l1_origin_number: u64,
) -> Result<DecodedTransaction, PipelineError> {
    let tx = OpTxEnvelope::decode(&mut &raw[..])
        .map_err(|e| PipelineError::TxDecode(format!("failed to decode tx: {}", e)))?;

    let hash = B256::from(*tx.tx_hash());
    let tx_type = tx.tx_type() as u8;
    let is_deposit = tx.tx_type() == OpTxType::Deposit;

    // Extract common fields based on tx type
    let (from, to, value, input, nonce, gas_limit) = match &tx {
        OpTxEnvelope::Legacy(signed) => {
            let inner = signed.tx();
            let from = signed.recover_signer().unwrap_or(Address::ZERO);
            (
                from,
                inner.to(),
                inner.value(),
                inner.input().clone(),
                inner.nonce(),
                inner.gas_limit(),
            )
        }
        OpTxEnvelope::Eip2930(signed) => {
            let inner = signed.tx();
            let from = signed.recover_signer().unwrap_or(Address::ZERO);
            (
                from,
                inner.to(),
                inner.value(),
                inner.input().clone(),
                inner.nonce(),
                inner.gas_limit(),
            )
        }
        OpTxEnvelope::Eip1559(signed) => {
            let inner = signed.tx();
            let from = signed.recover_signer().unwrap_or(Address::ZERO);
            (
                from,
                inner.to(),
                inner.value(),
                inner.input().clone(),
                inner.nonce(),
                inner.gas_limit(),
            )
        }
        OpTxEnvelope::Eip7702(signed) => {
            let inner = signed.tx();
            let from = signed.recover_signer().unwrap_or(Address::ZERO);
            (
                from,
                Some(inner.to), // EIP-7702 always has a `to` address
                inner.value(),
                inner.input().clone(),
                inner.nonce(),
                inner.gas_limit(),
            )
        }
        OpTxEnvelope::Deposit(deposit) => {
            // TxDeposit has `to` as TxKind, convert to Option<Address>
            let to = deposit.to.to().copied();
            (
                deposit.from,
                to,
                deposit.value,
                deposit.input.clone(),
                0u64, // Deposits don't have nonces
                deposit.gas_limit,
            )
        }
    };

    Ok(DecodedTransaction {
        hash,
        from,
        to,
        value,
        input,
        tx_type,
        nonce,
        gas_limit,
        l2_block_timestamp,
        l1_origin_number,
        is_deposit,
    })
}

/// Decode a deposit transaction from a DepositedTransaction struct.
///
/// This is used for deposits parsed from L1 events (not from raw bytes).
pub fn decode_deposit_tx(
    deposit: &DepositedTransaction,
    l2_block_timestamp: u64,
    l1_origin_number: u64,
) -> DecodedTransaction {
    DecodedTransaction {
        hash: deposit.tx_hash(),
        from: deposit.from,
        to: deposit.to,
        value: deposit.value,
        input: deposit.data.clone(),
        tx_type: 0x7E,
        nonce: 0,
        gas_limit: deposit.gas_limit,
        l2_block_timestamp,
        l1_origin_number,
        is_deposit: true,
    }
}
