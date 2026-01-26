//! Trait for abstracting the persistence API so different database backends can be used.
//!
//! We start with SQLite because it's easy and good.

mod sqlite;

pub use derivexex_types::{ChannelState, DerivationCheckpoint};
pub use sqlite::SqliteDb;

/// Trait for persisting derivation pipeline state.
/// Can be used to add support for different database backends.
pub trait DerivationDb: Send + Sync {
    /// Load the last checkpoint, or None if no checkpoint exists.
    fn load_checkpoint(&self) -> eyre::Result<Option<DerivationCheckpoint>>;

    /// Save a checkpoint atomically.
    fn save_checkpoint(&self, checkpoint: &DerivationCheckpoint) -> eyre::Result<()>;

    #[allow(dead_code)]
    fn load_pending_channels(&self) -> eyre::Result<Vec<ChannelState>>;

    /// Save a pending channel (upsert).
    #[allow(dead_code)]
    fn save_pending_channel(&self, channel: &ChannelState) -> eyre::Result<()>;

    /// Remove a channel (when complete or expired).
    #[allow(dead_code)]
    fn remove_channel(&self, channel_id: &[u8; 16]) -> eyre::Result<()>;

    fn clear_pending_channels(&self) -> eyre::Result<()>;

    /// Invalidate checkpoint if it references an L1 block at or after `first_invalid_block`.
    /// Returns true if checkpoint was deleted.
    fn invalidate_checkpoint_if_reorged(&self, first_invalid_block: u64) -> eyre::Result<bool>;
}
