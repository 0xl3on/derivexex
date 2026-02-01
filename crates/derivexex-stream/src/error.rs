//! Error types for the streaming pipeline.

use thiserror::Error;

use derivexex_pipeline::{ChannelError, DeriveError, FrameError};

/// Errors that can occur in the streaming pipeline.
#[derive(Debug, Error)]
pub enum PipelineError {
    /// WebSocket connection or communication error.
    #[error("websocket error: {0}")]
    WebSocket(String),

    /// Beacon API request error.
    #[error("beacon API error: {0}")]
    Beacon(String),

    /// Blob not found for the given slot and versioned hash.
    #[error("blob not found: slot={0}, hash={1}")]
    BlobNotFound(u64, String),

    /// Transaction decoding error.
    #[error("transaction decode error: {0}")]
    TxDecode(String),

    /// Signer recovery error.
    #[error("signer recovery error: {0}")]
    SignerRecovery(String),

    /// RLP decoding error.
    #[error("RLP decode error: {0}")]
    Rlp(String),

    /// Frame decoding error.
    #[error("frame error: {0}")]
    Frame(#[from] FrameError),

    /// Channel assembly or decompression error.
    #[error("channel error: {0}")]
    Channel(#[from] ChannelError),

    /// Derivation error.
    #[error("derive error: {0}")]
    Derive(#[from] DeriveError),

    /// HTTP request error.
    #[error("HTTP error: {0}")]
    Http(String),

    /// JSON parsing error.
    #[error("JSON error: {0}")]
    Json(String),

    /// L1 reorg detected.
    #[error("L1 reorg detected at block {0}")]
    L1Reorg(u64),

    /// Stream ended unexpectedly.
    #[error("stream ended unexpectedly")]
    StreamEnded,

    /// Invalid configuration.
    #[error("invalid configuration: {0}")]
    Config(String),
}

impl From<reqwest::Error> for PipelineError {
    fn from(e: reqwest::Error) -> Self {
        PipelineError::Http(e.to_string())
    }
}

impl From<serde_json::Error> for PipelineError {
    fn from(e: serde_json::Error) -> Self {
        PipelineError::Json(e.to_string())
    }
}
