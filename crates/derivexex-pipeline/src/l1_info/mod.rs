//! L1 Block Info transaction for OP Stack L2 blocks.
//!
//! Every L2 block starts with a special "L1 attributes" deposit transaction that
//! records L1 state (block number, timestamp, basefee, etc.). This is how L2 contracts
//! can access L1 context via the L1Block predeploy.
//!
//! The encoding format varies by hardfork:
//! - **Ecotone**: Packed `setL1BlockValuesEcotone` (164 bytes)
//! - **Isthmus**: Packed `setL1BlockValuesIsthmus` (180 bytes), adds operator fees
//!
//! Spec: https://specs.optimism.io/protocol/deposits.html#l1-attributes-deposited-transaction

mod encode;

use alloy_primitives::{address, keccak256, Address, Bytes, B256, U256};

use crate::deposits::DepositedTransaction;

pub use encode::{ECOTONE_L1_INFO_TX_CALLDATA_LEN, ISTHMUS_L1_INFO_TX_CALLDATA_LEN};

/// The following variables are hardcoded according to the OP stack:
/// https://specs.optimism.io/protocol/deposits.html#l1-attributes-deposited-transaction
/// The L1 attributes depositor account (EOA with no known private key).
pub const L1_ATTRIBUTES_DEPOSITOR: Address = address!("DeaDDEaDDeAdDeAdDEAdDEaddeAddEAdDEAd0001");
/// The L1Block predeploy contract address on L2.
pub const L1_BLOCK_ADDRESS: Address = address!("4200000000000000000000000000000000000015");
/// Gas limit for L1 attributes transaction (post-Regolith).
pub const L1_INFO_TX_GAS: u64 = 1_000_000;

/// OP Stack hardfork versions that affect derivation.
///
/// Each hardfork may change L1 attributes encoding, batch formats,
/// or derivation rules. Use this to select the correct encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Hardfork {
    /// Ecotone - EIP-4844 blobs (Mar 2024)
    /// Packed setL1BlockValuesEcotone, 164 bytes
    /// Current Unichain hard fork
    #[default]
    Ecotone,

    /// Isthmus - Operator fees (May 2025)
    /// Packed setL1BlockValuesIsthmus, 180 bytes
    /// Adds: operatorFeeScalar, operatorFeeConstant
    /// Unichain does not use it, but if they do eventually, I am ready
    Isthmus,
}

impl Hardfork {
    /// Returns the function selector for this hardfork's L1 info transaction.
    #[inline]
    pub const fn l1_info_selector(&self) -> [u8; 4] {
        match self {
            Self::Ecotone => encode::ECOTONE_SELECTOR,
            Self::Isthmus => encode::ISTHMUS_SELECTOR,
        }
    }

    /// Returns the expected calldata length for this hardfork.
    #[inline]
    pub const fn l1_info_calldata_len(&self) -> usize {
        match self {
            Self::Ecotone => ECOTONE_L1_INFO_TX_CALLDATA_LEN,
            Self::Isthmus => ISTHMUS_L1_INFO_TX_CALLDATA_LEN,
        }
    }

    /// Detect hardfork from L1 info calldata by checking the selector.
    pub fn from_l1_info_calldata(data: &[u8]) -> Option<Self> {
        if data.len() < 4 {
            return None;
        }
        let selector: [u8; 4] = data[..4].try_into().ok()?;
        match selector {
            encode::ECOTONE_SELECTOR => Some(Self::Ecotone),
            encode::ISTHMUS_SELECTOR => Some(Self::Isthmus),
            _ => None,
        }
    }
}

/// L1 block information used to construct the L1 attributes deposit transaction.
///
/// This data is extracted from L1 blocks and encoded into the first transaction
/// of each L2 block, allowing L2 contracts to access L1 context.
///
/// Field usage by hardfork:
/// - All fields except `operator_*`: Ecotone+
/// - `operator_fee_scalar`, `operator_fee_constant`: Isthmus+
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L1BlockInfo {
    /// L1 block number
    pub number: u64,
    /// L1 block timestamp
    pub timestamp: u64,
    /// L1 base fee per gas
    pub basefee: U256,
    /// L1 block hash
    pub hash: B256,
    /// L2 sequence number (block number within the epoch, starting at 0)
    pub sequence_number: u64,
    /// Address of the batch submitter
    pub batcher_addr: Address,
    /// Blob base fee (for EIP-4844)
    pub blob_basefee: U256,
    /// Base fee scalar (for L1 fee calculation)
    pub basefee_scalar: u32,
    /// Blob base fee scalar (for L1 fee calculation)
    pub blob_basefee_scalar: u32,

    // === Isthmus+ fields ===
    /// Operator fee scalar (Isthmus+)
    pub operator_fee_scalar: u32,
    /// Operator fee constant (Isthmus+)
    pub operator_fee_constant: u64,
}

impl Default for L1BlockInfo {
    fn default() -> Self {
        Self {
            number: 0,
            timestamp: 0,
            basefee: U256::ZERO,
            hash: B256::ZERO,
            sequence_number: 0,
            batcher_addr: Address::ZERO,
            blob_basefee: U256::ZERO,
            basefee_scalar: 0,
            blob_basefee_scalar: 0,
            operator_fee_scalar: 0,
            operator_fee_constant: 0,
        }
    }
}

impl L1BlockInfo {
    /// Create a new L1BlockInfo (Ecotone-compatible).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        number: u64,
        timestamp: u64,
        basefee: U256,
        hash: B256,
        sequence_number: u64,
        batcher_addr: Address,
        blob_basefee: U256,
        basefee_scalar: u32,
        blob_basefee_scalar: u32,
    ) -> Self {
        Self {
            number,
            timestamp,
            basefee,
            hash,
            sequence_number,
            batcher_addr,
            blob_basefee,
            basefee_scalar,
            blob_basefee_scalar,
            ..Default::default()
        }
    }

    /// Set Isthmus operator fee fields (builder pattern).
    #[inline]
    pub fn with_operator_fees(mut self, scalar: u32, constant: u64) -> Self {
        self.operator_fee_scalar = scalar;
        self.operator_fee_constant = constant;
        self
    }

    /// Encode the L1 block info as calldata for the specified hardfork.
    #[inline]
    pub fn encode(&self, hardfork: Hardfork, out: &mut Vec<u8>) {
        match hardfork {
            Hardfork::Ecotone => encode::encode_ecotone(self, out),
            Hardfork::Isthmus => encode::encode_isthmus(self, out),
        }
    }

    /// Encode the L1 block info and return as `Bytes`.
    pub fn to_calldata(&self, hardfork: Hardfork) -> Bytes {
        let mut buf = Vec::with_capacity(hardfork.l1_info_calldata_len());
        self.encode(hardfork, &mut buf);
        Bytes::from(buf)
    }

    /// Compute the source hash for this L1 attributes transaction.
    ///
    /// Source hash = keccak256(domain=1 || keccak256(l1BlockHash || sequenceNumber))
    pub fn source_hash(&self) -> B256 {
        compute_l1_info_source_hash(self.hash, self.sequence_number)
    }

    /// Convert this L1BlockInfo into a deposit transaction for the specified hardfork.
    ///
    /// This creates the L1 attributes deposited transaction that must be
    /// the first transaction in every L2 block.
    /// Example: https://unichain.blockscout.com/tx/0x0c5cbaf6631111c2747333f21a781ccdce3e6c4529f0fc68e678a558d3d859f1
    pub fn to_deposit_tx(&self, hardfork: Hardfork) -> DepositedTransaction {
        DepositedTransaction {
            source_hash: self.source_hash(),
            from: L1_ATTRIBUTES_DEPOSITOR,
            to: Some(L1_BLOCK_ADDRESS),
            mint: U256::ZERO,
            value: U256::ZERO,
            gas_limit: L1_INFO_TX_GAS,
            is_system_tx: true,
            data: self.to_calldata(hardfork),
        }
    }
}

/// Compute source hash for L1 attributes transaction.
///
/// Uses domain = 1 (vs 0 for user deposits):
/// `keccak256(bytes32(1) || keccak256(l1BlockHash || bytes32(sequenceNumber)))`
pub fn compute_l1_info_source_hash(l1_block_hash: B256, sequence_number: u64) -> B256 {
    // Inner hash: keccak256(l1BlockHash || sequenceNumber)
    let mut deposit_id_input = [0u8; 64];
    deposit_id_input[..32].copy_from_slice(l1_block_hash.as_slice());
    deposit_id_input[56..64].copy_from_slice(&sequence_number.to_be_bytes());
    let deposit_id = keccak256(deposit_id_input);

    // Outer hash with domain = 1 (L1 attributes)
    let mut source_hash_input = [0u8; 64];
    source_hash_input[31] = 1; // domain = 1
    source_hash_input[32..64].copy_from_slice(deposit_id.as_slice());
    keccak256(source_hash_input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hardfork_default_is_ecotone() {
        assert_eq!(Hardfork::default(), Hardfork::Ecotone);
    }

    #[test]
    fn test_hardfork_selectors() {
        assert_eq!(Hardfork::Ecotone.l1_info_selector(), [0x44, 0x0a, 0x5e, 0x20]);
        assert_eq!(Hardfork::Isthmus.l1_info_selector(), [0x09, 0x8f, 0x40, 0x81]);
    }

    #[test]
    fn test_hardfork_calldata_lengths() {
        assert_eq!(Hardfork::Ecotone.l1_info_calldata_len(), 164);
        assert_eq!(Hardfork::Isthmus.l1_info_calldata_len(), 180);
    }

    #[test]
    fn test_detect_hardfork_from_calldata() {
        let info = L1BlockInfo::default();

        let ecotone_data = info.to_calldata(Hardfork::Ecotone);
        assert_eq!(Hardfork::from_l1_info_calldata(&ecotone_data), Some(Hardfork::Ecotone));

        let isthmus_data = info.to_calldata(Hardfork::Isthmus);
        assert_eq!(Hardfork::from_l1_info_calldata(&isthmus_data), Some(Hardfork::Isthmus));
    }

    #[test]
    fn test_source_hash_uses_domain_1() {
        let hash = B256::repeat_byte(0xab);
        let seq = 5u64;

        let source_hash = compute_l1_info_source_hash(hash, seq);

        assert_ne!(source_hash, B256::ZERO);
        assert_eq!(source_hash, compute_l1_info_source_hash(hash, seq));

        // Different sequence number should give different hash
        let source_hash_2 = compute_l1_info_source_hash(hash, 6);
        assert_ne!(source_hash, source_hash_2);
    }

    #[test]
    fn test_source_hash_verification_block_1008() {
        // This verifies the source_hash for block 1008 in the l2_blocks.json test output
        // l1_block_hash = 0x8a8a8a8a... (mock hash from test)
        // sequence_number = 8
        let l1_block_hash = B256::repeat_byte(0x8a);
        let sequence_number = 8u64;

        let source_hash = compute_l1_info_source_hash(l1_block_hash, sequence_number);

        // This is the expected source_hash from l2_blocks.json block 1008
        let expected: B256 =
            "0x2c7fadd35b795e92bbcce242429d49fa2adff2e189ce68e8c42cb72f95b1ca9b".parse().unwrap();

        assert_eq!(source_hash, expected, "source_hash should match JSON output");
    }

    #[test]
    fn test_to_deposit_tx_ecotone() {
        let info = L1BlockInfo::new(
            12345,
            1700000000,
            U256::from(1_000_000_000u64),
            B256::repeat_byte(0x11),
            0,
            address!("2F60A5184c63ca94f82a27100643DbAbe4F3f7Fd"),
            U256::from(1u64),
            1000,
            500,
        );

        let deposit = info.to_deposit_tx(Hardfork::Ecotone);

        assert_eq!(deposit.from, L1_ATTRIBUTES_DEPOSITOR);
        assert_eq!(deposit.to, Some(L1_BLOCK_ADDRESS));
        assert_eq!(deposit.mint, U256::ZERO);
        assert_eq!(deposit.value, U256::ZERO);
        assert_eq!(deposit.gas_limit, L1_INFO_TX_GAS);
        assert!(deposit.is_system_tx);
        assert_eq!(deposit.data.len(), ECOTONE_L1_INFO_TX_CALLDATA_LEN);
    }

    #[test]
    fn test_to_deposit_tx_isthmus() {
        let info = L1BlockInfo::new(
            12345,
            1700000000,
            U256::from(1_000_000_000u64),
            B256::repeat_byte(0x11),
            0,
            address!("2F60A5184c63ca94f82a27100643DbAbe4F3f7Fd"),
            U256::from(1u64),
            1000,
            500,
        )
        .with_operator_fees(100, 200);

        let deposit = info.to_deposit_tx(Hardfork::Isthmus);

        assert_eq!(deposit.data.len(), ISTHMUS_L1_INFO_TX_CALLDATA_LEN);
        assert_eq!(&deposit.data[..4], &[0x09, 0x8f, 0x40, 0x81]);
    }

    #[test]
    fn test_builder_pattern() {
        let info =
            L1BlockInfo::new(1, 2, U256::ZERO, B256::ZERO, 0, Address::ZERO, U256::ZERO, 0, 0)
                .with_operator_fees(123, 456);

        assert_eq!(info.operator_fee_scalar, 123);
        assert_eq!(info.operator_fee_constant, 456);
    }

    #[test]
    fn test_addresses() {
        assert_eq!(L1_ATTRIBUTES_DEPOSITOR, address!("DeaDDEaDDeAdDeAdDEAdDEaddeAddEAdDEAd0001"));
        assert_eq!(L1_BLOCK_ADDRESS, address!("4200000000000000000000000000000000000015"));
    }
}
