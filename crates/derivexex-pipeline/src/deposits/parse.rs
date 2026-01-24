//! Parsing logic for `TransactionDeposited` events.

use alloy_primitives::{keccak256, Bytes, B256, U256};
use alloy_sol_types::{sol, SolEvent};

use super::{DepositError, DepositedTransaction};

sol! {
    /// The `TransactionDeposited` event emitted by OptimismPortal on L1.
    #[derive(Debug)]
    event TransactionDeposited(
        address indexed from,
        address indexed to,
        uint256 indexed version,
        bytes opaqueData
    );
}

/// Keccak256 hash of `TransactionDeposited(address,address,uint256,bytes)`.
pub const TRANSACTION_DEPOSITED_TOPIC: B256 = B256::new(TransactionDeposited::SIGNATURE_HASH.0);

/// Parse a `TransactionDeposited` event log into a `DepositedTransaction`.
pub fn from_log(
    log_data: &[u8],
    topics: &[B256],
    l1_block_hash: B256,
    log_index: u64,
) -> Result<DepositedTransaction, DepositError> {
    let log = TransactionDeposited::decode_raw_log(topics.iter().copied(), log_data)?;

    // Only version 0 is supported
    if log.version != U256::ZERO {
        return Err(DepositError::UnsupportedVersion(log.version));
    }

    // opaqueData layout (version 0):
    // [0..32)   mint (uint256)
    // [32..64)  value (uint256)
    // [64..72)  gas_limit (uint64)
    // [72]      is_creation (bool)
    // [73..)    data (bytes)
    // Source: https://github.com/ethereum-optimism/optimism/blob/111f3f3a3a2881899662e53e0f1b2f845b188a38/packages/contracts-bedrock/src/L1/OptimismPortal.sol#L414
    const MIN_OPAQUE_LEN: usize = 73;
    let opaque: &[u8] = log.opaqueData.as_ref();

    if opaque.len() < MIN_OPAQUE_LEN {
        return Err(DepositError::InvalidOpaqueDataLength(opaque.len()));
    }

    let mint = U256::from_be_slice(&opaque[0..32]);
    let value = U256::from_be_slice(&opaque[32..64]);

    // SAFETY: slice is exactly 8 bytes
    let gas_limit = u64::from_be_bytes(opaque[64..72].try_into().unwrap());

    let is_creation = opaque[72] != 0;

    let data = if opaque.len() > MIN_OPAQUE_LEN {
        Bytes::copy_from_slice(&opaque[MIN_OPAQUE_LEN..])
    } else {
        Bytes::new()
    };

    let to = if is_creation { None } else { Some(log.to) };

    let source_hash = compute_source_hash(l1_block_hash, log_index);

    Ok(DepositedTransaction {
        source_hash,
        from: log.from,
        to,
        mint,
        value,
        gas_limit,
        is_system_tx: false,
        data,
    })
}

/// Compute the source hash for a user-deposited transaction.
///
/// Formula: `keccak256(bytes32(0) || keccak256(l1_block_hash || bytes32(log_index)))`
///
/// The leading zero byte acts as a domain separator (0 = user deposit, 1 = L1 attributes, 2 =
/// upgrade).
pub fn compute_source_hash(l1_block_hash: B256, log_index: u64) -> B256 {
    // deposit_id = keccak256(l1_block_hash || log_index as bytes32)
    let mut deposit_id_input = [0u8; 64];
    deposit_id_input[..32].copy_from_slice(l1_block_hash.as_slice());
    deposit_id_input[56..64].copy_from_slice(&log_index.to_be_bytes());

    let deposit_id = keccak256(deposit_id_input);

    // source_hash = keccak256(bytes32(0) || deposit_id)
    // Domain 0 = user-deposited transaction
    let mut source_hash_input = [0u8; 64];
    // First 32 bytes are zero (domain separator)
    source_hash_input[32..64].copy_from_slice(deposit_id.as_slice());

    keccak256(source_hash_input)
}
