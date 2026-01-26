//! SQLite implementation of DerivationDb.

use super::{ChannelState, DerivationCheckpoint, DerivationDb};
use eyre::{eyre, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::{path::Path, sync::Mutex};

/// SQLite-backed persistence for derivation state.
pub struct SqliteDb {
    conn: Mutex<Connection>,
}

impl SqliteDb {
    /// Open or create a SQLite database at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn: Mutex::new(conn) };
        db.init_schema()?;
        Ok(db)
    }

    /// Create an in-memory database (useful for testing).
    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn: Mutex::new(conn) };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| eyre!("lock poisoned: {e}"))?;

        // Create tables if they don't exist
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS checkpoint (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                l1_block_number INTEGER NOT NULL,
                l1_block_hash BLOB NOT NULL,
                l2_blocks_derived INTEGER NOT NULL,
                l2_txs_derived INTEGER NOT NULL,
                timestamp INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS pending_channels (
                channel_id BLOB PRIMARY KEY,
                data BLOB NOT NULL,
                opened_at_l1_block INTEGER NOT NULL
            );
            "#,
        )?;

        // Migration: add next_l2_block_number column if it doesn't exist
        // SQLite doesn't have ADD COLUMN IF NOT EXISTS, so we check first
        let has_column: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('checkpoint') WHERE name='next_l2_block_number'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0) > 0;

        if !has_column {
            conn.execute(
                "ALTER TABLE checkpoint ADD COLUMN next_l2_block_number INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }

        Ok(())
    }
}

impl DerivationDb for SqliteDb {
    fn load_checkpoint(&self) -> Result<Option<DerivationCheckpoint>> {
        let conn = self.conn.lock().map_err(|e| eyre!("lock poisoned: {e}"))?;

        let result: Option<(i64, Vec<u8>, i64, i64, i64, i64)> = conn
            .query_row(
                "SELECT l1_block_number, l1_block_hash, next_l2_block_number, l2_blocks_derived, l2_txs_derived, timestamp
                 FROM checkpoint WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )
            .optional()?;

        match result {
            Some((block_num, hash_bytes, next_l2, l2_blocks, l2_txs, ts)) => {
                let mut l1_block_hash = [0u8; 32];
                if hash_bytes.len() == 32 {
                    l1_block_hash.copy_from_slice(&hash_bytes);
                }

                Ok(Some(DerivationCheckpoint {
                    l1_block_number: block_num as u64,
                    l1_block_hash,
                    next_l2_block_number: next_l2 as u64,
                    l2_blocks_derived: l2_blocks as u64,
                    l2_txs_derived: l2_txs as u64,
                    timestamp: ts as u64,
                }))
            }
            None => Ok(None),
        }
    }

    fn save_checkpoint(&self, checkpoint: &DerivationCheckpoint) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| eyre!("lock poisoned: {e}"))?;

        conn.execute(
            "INSERT OR REPLACE INTO checkpoint
             (id, l1_block_number, l1_block_hash, next_l2_block_number, l2_blocks_derived, l2_txs_derived, timestamp)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                checkpoint.l1_block_number as i64,
                checkpoint.l1_block_hash.as_slice(),
                checkpoint.next_l2_block_number as i64,
                checkpoint.l2_blocks_derived as i64,
                checkpoint.l2_txs_derived as i64,
                checkpoint.timestamp as i64,
            ],
        )?;

        Ok(())
    }

    fn load_pending_channels(&self) -> Result<Vec<ChannelState>> {
        let conn = self.conn.lock().map_err(|e| eyre!("lock poisoned: {e}"))?;

        let mut stmt = conn.prepare("SELECT data FROM pending_channels")?;
        let rows = stmt.query_map([], |row| {
            let data: Vec<u8> = row.get(0)?;
            Ok(data)
        })?;

        let mut channels = Vec::new();
        for row in rows {
            let data = row?;
            if let Ok(channel) = serde_json::from_slice::<ChannelState>(&data) {
                channels.push(channel);
            }
        }

        Ok(channels)
    }

    fn save_pending_channel(&self, channel: &ChannelState) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| eyre!("lock poisoned: {e}"))?;

        let data = serde_json::to_vec(channel)?;

        conn.execute(
            "INSERT OR REPLACE INTO pending_channels (channel_id, data, opened_at_l1_block)
             VALUES (?1, ?2, ?3)",
            params![channel.id.as_slice(), data, channel.opened_at_l1_block as i64,],
        )?;

        Ok(())
    }

    fn remove_channel(&self, channel_id: &[u8; 16]) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| eyre!("lock poisoned: {e}"))?;

        conn.execute(
            "DELETE FROM pending_channels WHERE channel_id = ?1",
            params![channel_id.as_slice()],
        )?;

        Ok(())
    }

    fn clear_pending_channels(&self) -> Result<()> {
        let conn = self.conn.lock().map_err(|e| eyre!("lock poisoned: {e}"))?;

        conn.execute("DELETE FROM pending_channels", [])?;

        Ok(())
    }

    fn invalidate_checkpoint_if_reorged(&self, first_invalid_block: u64) -> Result<bool> {
        let conn = self.conn.lock().map_err(|e| eyre!("lock poisoned: {e}"))?;

        // Check if checkpoint exists and is at or after the invalid block
        let checkpoint_block: Option<i64> = conn
            .query_row("SELECT l1_block_number FROM checkpoint WHERE id = 1", [], |row| row.get(0))
            .optional()?;

        if let Some(block_num) = checkpoint_block {
            if block_num as u64 >= first_invalid_block {
                conn.execute("DELETE FROM checkpoint WHERE id = 1", [])?;
                return Ok(true);
            }
        }

        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_roundtrip() {
        let db = SqliteDb::in_memory().unwrap();

        // Initially empty
        assert!(db.load_checkpoint().unwrap().is_none());

        // Save checkpoint
        let checkpoint = DerivationCheckpoint {
            l1_block_number: 12345,
            l1_block_hash: [0xab; 32],
            next_l2_block_number: 67890,
            l2_blocks_derived: 100,
            l2_txs_derived: 500,
            timestamp: 1234567890,
        };
        db.save_checkpoint(&checkpoint).unwrap();

        // Load it back
        let loaded = db.load_checkpoint().unwrap().unwrap();
        assert_eq!(loaded.l1_block_number, 12345);
        assert_eq!(loaded.l1_block_hash, [0xab; 32]);
        assert_eq!(loaded.next_l2_block_number, 67890);
        assert_eq!(loaded.l2_blocks_derived, 100);
        assert_eq!(loaded.l2_txs_derived, 500);
    }

    #[test]
    fn test_pending_channels() {
        let db = SqliteDb::in_memory().unwrap();

        let channel = ChannelState {
            id: [0x01; 16],
            frames: vec![],
            is_closed: false,
            highest_frame: 0,
            opened_at_l1_block: 100,
        };

        db.save_pending_channel(&channel).unwrap();

        let channels = db.load_pending_channels().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].id, [0x01; 16]);

        db.remove_channel(&[0x01; 16]).unwrap();

        let channels = db.load_pending_channels().unwrap();
        assert!(channels.is_empty());
    }

    #[test]
    fn test_invalidate_checkpoint_on_reorg() {
        let db = SqliteDb::in_memory().unwrap();

        // Save checkpoint at block 100
        let checkpoint = DerivationCheckpoint {
            l1_block_number: 100,
            l1_block_hash: [0xab; 32],
            next_l2_block_number: 500,
            l2_blocks_derived: 50,
            l2_txs_derived: 200,
            timestamp: 1234567890,
        };
        db.save_checkpoint(&checkpoint).unwrap();

        // Reorg starting at block 150 - checkpoint should survive
        assert!(!db.invalidate_checkpoint_if_reorged(150).unwrap());
        assert!(db.load_checkpoint().unwrap().is_some());

        // Reorg starting at block 100 - checkpoint should be deleted
        assert!(db.invalidate_checkpoint_if_reorged(100).unwrap());
        assert!(db.load_checkpoint().unwrap().is_none());

        // Save new checkpoint at block 200
        let checkpoint2 = DerivationCheckpoint { l1_block_number: 200, ..checkpoint };
        db.save_checkpoint(&checkpoint2).unwrap();

        // Reorg starting at block 50 - checkpoint should be deleted
        assert!(db.invalidate_checkpoint_if_reorged(50).unwrap());
        assert!(db.load_checkpoint().unwrap().is_none());
    }
}
