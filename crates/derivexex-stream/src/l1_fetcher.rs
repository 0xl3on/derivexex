//! L1 block info fetcher for missing epochs.

use alloy_primitives::{B256, U256};
use reqwest::Client;

use derivexex_pipeline::{EpochInfo, L1BlockRef};

use super::PipelineError;

/// Fetches L1 block info via JSON-RPC.
pub struct L1Fetcher {
    client: Client,
    rpc_url: String,
}

impl L1Fetcher {
    /// Create a new L1 fetcher from a WebSocket URL.
    /// Converts wss:// to https:// and ws:// to http://.
    pub fn new(ws_url: &str) -> Self {
        let rpc_url = ws_url.replace("wss://", "https://").replace("ws://", "http://");
        Self { client: Client::new(), rpc_url }
    }

    /// Fetch L1 block info by block number.
    pub async fn get_block_info(&self, block_number: u64) -> Result<EpochInfo, PipelineError> {
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_getBlockByNumber",
            "params": [format!("0x{:x}", block_number), false]
        });

        let resp = self
            .client
            .post(&self.rpc_url)
            .json(&req)
            .send()
            .await
            .map_err(|e| PipelineError::Http(e.to_string()))?;

        let json: serde_json::Value =
            resp.json().await.map_err(|e| PipelineError::Json(e.to_string()))?;

        let result = json
            .get("result")
            .ok_or_else(|| PipelineError::Http("No result in response".to_string()))?;

        let hash = result
            .get("hash")
            .and_then(|h| h.as_str())
            .and_then(|s| s.parse::<B256>().ok())
            .ok_or_else(|| PipelineError::Http("Missing block hash".to_string()))?;

        let timestamp = result
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .ok_or_else(|| PipelineError::Http("Missing timestamp".to_string()))?;

        let basefee = result
            .get("baseFeePerGas")
            .and_then(|b| b.as_str())
            .and_then(|s| U256::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .unwrap_or(U256::ZERO);

        let excess_blob_gas = result
            .get("excessBlobGas")
            .and_then(|b| b.as_str())
            .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
            .unwrap_or(0);

        // Calculate blob base fee from excess blob gas (EIP-4844)
        let blob_basefee = calc_blob_basefee(excess_blob_gas);

        Ok(EpochInfo {
            l1_ref: L1BlockRef { number: block_number, hash, timestamp },
            basefee,
            blob_basefee: U256::from(blob_basefee),
        })
    }

    // /// Fetch multiple L1 blocks in a range.
    // pub async fn get_block_range(
    //     &self,
    //     start: u64,
    //     end: u64,
    // ) -> Result<Vec<EpochInfo>, PipelineError> {
    //     let mut epochs = Vec::with_capacity((end - start + 1) as usize);
    //     for block_num in start..=end {
    //         let info = self.get_block_info(block_num).await?;
    //         epochs.push(info);
    //     }
    //     Ok(epochs)
    // }
}

/// Calculate blob base fee from excess blob gas (EIP-4844).
/// blob_basefee = MIN_BLOB_GASPRICE * e^(excess_blob_gas / BLOB_GASPRICE_UPDATE_FRACTION)
fn calc_blob_basefee(excess_blob_gas: u64) -> u128 {
    const MIN_BLOB_GASPRICE: u128 = 1;
    const BLOB_GASPRICE_UPDATE_FRACTION: u128 = 3338477;

    if excess_blob_gas == 0 {
        return MIN_BLOB_GASPRICE;
    }

    // Approximation using integer math
    let excess = excess_blob_gas as u128;
    let mut result = MIN_BLOB_GASPRICE;

    // e^x ≈ (1 + x/n)^n for small x/n
    // Using a simple approximation: result = 1 * e^(excess / fraction)
    let factor = (excess * 1_000_000) / BLOB_GASPRICE_UPDATE_FRACTION;
    result = result * (1_000_000 + factor) / 1_000_000;

    result.max(MIN_BLOB_GASPRICE)
}
