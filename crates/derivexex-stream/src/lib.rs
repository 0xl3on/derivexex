//! Async streaming pipeline for Unichain derivation.
//!
//! This module provides real-time L2 block derivation by:
//! - Following L1 via WebSocket subscription
//! - Fetching blobs from the Beacon API
//! - Decoding transactions and recovering signers
//! - Detecting and handling L1 reorgs
//! - Tracking safe/finalized heads
//!
//! # Quick Start
//!
//! ```ignore
//! use derivexex_stream::{run_pipeline, PipelineEvent};
//! use futures::StreamExt;
//!
//! // Start with defaults (Unichain mainnet)
//! let pipeline = run_pipeline().await?;
//!
//! // Stream L2 blocks and events
//! let mut events = pipeline.stream_l2_blocks();
//! while let Some(event) = events.next().await {
//!     match event? {
//!         PipelineEvent::Block(block) => println!("L2 #{}", block.number),
//!         PipelineEvent::Reorg(reorg) => println!("Reorg at {}", reorg.first_invalid),
//!         PipelineEvent::HeadUpdate(heads) => println!("Safe: {}", heads.safe_head),
//!     }
//! }
//! ```
//!
//! # Custom Configuration
//!
//! ```ignore
//! use derivexex_stream::UnichainPipeline;
//!
//! let pipeline = UnichainPipeline::builder()
//!     .l1_ws_url("wss://custom-node.com")
//!     .beacon_url("http://custom-beacon.com")
//!     .blob_cache_capacity(256)
//!     .build()    // Returns UnichainPipelineConfig
//!     .start()    // Async - initializes and returns pipeline
//!     .await?;
//! ```

mod blob_fetcher;
mod error;
mod heads;
mod l1_fetcher;
mod l1_follower;
mod pipeline;
mod reorg;
mod tx_decoder;

#[cfg(test)]
mod tests;

pub use error::PipelineError;
pub use heads::HeadUpdate;
pub use pipeline::{PipelineEvent, UnichainPipeline, UnichainPipelineConfig};
pub use reorg::ReorgEvent;
pub use tx_decoder::DecodedTransaction;

/// Start a Unichain pipeline with default configuration.
///
/// This is the simplest way to start streaming L2 blocks.
/// Uses default Unichain mainnet endpoints.
///
/// # Example
///
/// ```ignore
/// let pipeline = run_pipeline().await?;
/// pipeline.run().await?; // Blocks, processing L1 and emitting L2 blocks
/// ```
pub async fn run_pipeline() -> Result<UnichainPipeline, PipelineError> {
    UnichainPipeline::builder().build().start().await
}
