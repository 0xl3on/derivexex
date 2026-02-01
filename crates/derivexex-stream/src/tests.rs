//! Tests for the streaming module.

use alloy_primitives::{Address, Bytes, B256, U256};

use super::{heads::HeadTracker, reorg::ReorgDetector, tx_decoder::DecodedTransaction};

fn hash(n: u8) -> B256 {
    let mut bytes = [0u8; 32];
    bytes[0] = n;
    B256::from(bytes)
}

// =============================================================================
// HeadTracker tests
// =============================================================================

#[test]
fn test_head_tracking() {
    let mut tracker = HeadTracker::new(4); // 4 for finalized

    // Add L2 blocks: (l2_number, l1_origin, l1_inclusion)
    // L2 blocks 100-104 with L1 origins 10-14, all included in same L1 blocks
    tracker.add_l2_block(100, 10, 10);
    tracker.add_l2_block(101, 11, 11);
    tracker.add_l2_block(102, 12, 12);
    tracker.add_l2_block(103, 13, 13);
    tracker.add_l2_block(104, 14, 14);

    // L1 head at 16: finalized cutoff = 12
    let heads = tracker.update_l1_head(16);

    assert_eq!(heads.unsafe_head, 104);
    assert_eq!(heads.safe_head, 104); // All derived blocks are safe (on L1)
    assert_eq!(heads.finalized_head, 102); // L1 inclusion 12 <= 12
}

#[test]
fn test_head_tracker_reorg_handling() {
    let mut tracker = HeadTracker::new(4);

    // L2 blocks with l1_inclusion values
    tracker.add_l2_block(100, 10, 10);
    tracker.add_l2_block(101, 11, 11);
    tracker.add_l2_block(102, 12, 12);
    tracker.add_l2_block(103, 13, 13);

    // Reorg at L1 block 12 - removes blocks with l1_inclusion >= 12
    tracker.handle_reorg(12);

    let heads = tracker.heads();
    assert_eq!(heads.unsafe_head, 101); // Only blocks with l1_inclusion < 12 remain
}

// =============================================================================
// ReorgDetector tests
// =============================================================================

#[test]
fn test_no_reorg() {
    let mut detector = ReorgDetector::new(10);

    // Process blocks in order
    assert!(detector.process_block(1, hash(1), hash(0)).is_none());
    assert!(detector.process_block(2, hash(2), hash(1)).is_none());
    assert!(detector.process_block(3, hash(3), hash(2)).is_none());

    assert_eq!(detector.latest_block(), Some(3));
}

#[test]
fn test_reorg_detected() {
    let mut detector = ReorgDetector::new(10);

    // Process blocks 1-3
    detector.process_block(1, hash(1), hash(0));
    detector.process_block(2, hash(2), hash(1));
    detector.process_block(3, hash(3), hash(2));

    // Block 4 with wrong parent (reorg at block 3)
    let reorg = detector.process_block(4, hash(14), hash(12)); // parent is hash(12), not hash(3)

    // This should detect a reorg since hash(12) != hash(3)
    assert!(reorg.is_some());
    let event = reorg.unwrap();
    assert_eq!(event.detected_at, 4);
}

#[test]
fn test_old_block_reorg() {
    let mut detector = ReorgDetector::new(10);

    detector.process_block(1, hash(1), hash(0));
    detector.process_block(2, hash(2), hash(1));
    detector.process_block(3, hash(3), hash(2));

    // Receive block 2 again with different hash (reorg)
    let reorg = detector.process_block(2, hash(22), hash(1));

    assert!(reorg.is_some());
    let event = reorg.unwrap();
    assert_eq!(event.first_invalid, 2);
    assert_eq!(event.old_hash, hash(2));
    assert_eq!(event.new_hash, hash(22));
}

// =============================================================================
// TxDecoder tests
// =============================================================================

#[test]
fn test_decoded_transaction_struct() {
    let decoded = DecodedTransaction {
        hash: B256::ZERO,
        from: Address::ZERO,
        to: Some(Address::ZERO),
        value: U256::ZERO,
        input: Bytes::new(),
        tx_type: 0,
        nonce: 1,
        gas_limit: 21000,
        l2_block_timestamp: 1700000000,
        l1_origin_number: 10,
        is_deposit: false,
    };

    assert!(!decoded.is_deposit);
    assert_eq!(decoded.tx_type, 0);
}
