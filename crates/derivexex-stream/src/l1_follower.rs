//! L1 block follower via WebSocket subscription.

use std::pin::Pin;

use alloy_primitives::{Address, B256, U256};
use futures::{stream::Stream, SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use derivexex_pipeline::{
    config::{BATCHER, BATCH_INBOX, OPTIMISM_PORTAL},
    deposits::TRANSACTION_DEPOSITED_TOPIC,
    DepositedTransaction,
};

use super::PipelineError;

/// Events emitted by the L1 follower.
#[derive(Debug, Clone)]
pub enum L1Event {
    /// New L1 block header received.
    NewBlock(L1BlockHeader),
    /// Batch transaction detected (to BATCH_INBOX from BATCHER).
    BatchTx(BatchTransaction),
    /// Deposit event detected from OptimismPortal.
    Deposit(DepositInfo),
}

/// L1 block header from newHeads subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct L1BlockHeader {
    pub number: U256,
    pub hash: B256,
    pub parent_hash: B256,
    pub timestamp: U256,
    pub base_fee_per_gas: Option<U256>,
    #[serde(default)]
    pub excess_blob_gas: Option<U256>,
}

impl L1BlockHeader {
    /// Get block number as u64.
    #[inline]
    pub fn block_number(&self) -> u64 {
        self.number.try_into().unwrap_or(0)
    }
}

/// A batch transaction sent to the batch inbox.
#[derive(Debug, Clone)]
pub struct BatchTransaction {
    pub block_number: u64,
    pub block_hash: B256,
    pub block_timestamp: u64,
    pub blob_hashes: Vec<B256>,
}

/// Information about a deposit event.
#[derive(Debug, Clone)]
pub struct DepositInfo {
    pub l1_block_number: u64,
    pub deposit: DepositedTransaction,
}

/// Follows L1 blocks via WebSocket and emits relevant events.
pub struct L1Follower {
    ws_url: String,
    batch_inbox: Address,
    batcher: Address,
    portal: Address,
}

impl L1Follower {
    /// Create a new L1 follower with default Unichain addresses.
    ///
    /// Automatically converts `https://` to `wss://` and `http://` to `ws://`.
    pub fn new(ws_url: impl Into<String>) -> Self {
        let url = ws_url.into();
        let ws_url = url.replace("https://", "wss://").replace("http://", "ws://");
        Self { ws_url, batch_inbox: BATCH_INBOX, batcher: BATCHER, portal: OPTIMISM_PORTAL }
    }

    /// Subscribe to L1 events.
    ///
    /// Returns a stream of L1 events (new blocks, batch transactions, deposits).
    pub async fn subscribe(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<L1Event, PipelineError>> + Send>>, PipelineError>
    {
        let (tx, rx) = mpsc::channel(256);
        let ws_url = self.ws_url.clone();
        let batch_inbox = self.batch_inbox;
        let batcher = self.batcher;
        let portal = self.portal;

        tokio::spawn(async move {
            if let Err(e) = run_subscription(ws_url, batch_inbox, batcher, portal, tx).await {
                tracing::error!("L1 follower error: {}", e);
            }
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

async fn run_subscription(
    ws_url: String,
    batch_inbox: Address,
    batcher: Address,
    portal: Address,
    tx: mpsc::Sender<Result<L1Event, PipelineError>>,
) -> Result<(), PipelineError> {
    let (ws_stream, _) =
        connect_async(&ws_url).await.map_err(|e| PipelineError::WebSocket(e.to_string()))?;

    let (mut write, mut read) = ws_stream.split();

    // Subscribe to newHeads
    let subscribe_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_subscribe",
        "params": ["newHeads"]
    });

    write
        .send(Message::Text(subscribe_msg.to_string().into()))
        .await
        .map_err(|e| PipelineError::WebSocket(e.to_string()))?;

    // Process incoming messages
    while let Some(msg) = read.next().await {
        let msg = match msg {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(e) => {
                let _ = tx.send(Err(PipelineError::WebSocket(e.to_string()))).await;
                break;
            }
        };

        // Parse the message
        let parsed: serde_json::Value = match serde_json::from_str(&msg) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Check for subscription result (newHeads notification)
        if let Some(params) = parsed.get("params") {
            if let Some(result) = params.get("result") {
                if let Ok(header) = serde_json::from_value::<L1BlockHeader>(result.clone()) {
                    // Emit the new block event
                    let block_number = header.block_number();
                    let block_hash = header.hash;

                    if tx.send(Ok(L1Event::NewBlock(header))).await.is_err() {
                        break;
                    }

                    // Fetch block details to get transactions and receipts
                    // This would need an HTTP client for eth_getBlockByHash and eth_getLogs
                    // For now, we spawn a task to fetch the details
                    let tx_clone = tx.clone();
                    let ws_url_clone = ws_url.clone();
                    tokio::spawn(async move {
                        if let Err(e) = fetch_block_details(
                            &ws_url_clone,
                            block_number,
                            block_hash,
                            batch_inbox,
                            batcher,
                            portal,
                            tx_clone,
                        )
                        .await
                        {
                            tracing::warn!("Failed to fetch block details: {}", e);
                        }
                    });
                }
            }
        }
    }

    Ok(())
}

async fn fetch_block_details(
    _ws_url: &str,
    block_number: u64,
    block_hash: B256,
    batch_inbox: Address,
    batcher: Address,
    portal: Address,
    tx: mpsc::Sender<Result<L1Event, PipelineError>>,
) -> Result<(), PipelineError> {
    // We need to use an HTTP endpoint for these calls
    // Convert WS URL to HTTP
    let http_url = _ws_url.replace("wss://", "https://").replace("ws://", "http://");

    let client = reqwest::Client::new();

    // Fetch block with transactions
    let block_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_getBlockByHash",
        "params": [format!("{:?}", block_hash), true]
    });

    let block_resp = client
        .post(&http_url)
        .json(&block_req)
        .send()
        .await
        .map_err(|e| PipelineError::Http(e.to_string()))?;

    let block_json: serde_json::Value =
        block_resp.json().await.map_err(|e| PipelineError::Json(e.to_string()))?;

    // Extract block timestamp
    let block_timestamp = block_json
        .get("result")
        .and_then(|r| r.get("timestamp"))
        .and_then(|t| t.as_str())
        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);

    // Process transactions
    if let Some(txs) =
        block_json.get("result").and_then(|r| r.get("transactions")).and_then(|t| t.as_array())
    {
        for tx_val in txs {
            let to =
                tx_val.get("to").and_then(|t| t.as_str()).and_then(|s| s.parse::<Address>().ok());
            let from =
                tx_val.get("from").and_then(|f| f.as_str()).and_then(|s| s.parse::<Address>().ok());
            let tx_type = tx_val
                .get("type")
                .and_then(|t| t.as_str())
                .map(|s| u8::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0));

            // Check if this is a batch transaction
            if to == Some(batch_inbox) && from == Some(batcher) {
                tracing::info!("Found batch tx in L1 block {} (type {:?})", block_number, tx_type);
                // Extract blob hashes for type-3 transactions
                let blob_hashes = if tx_type == Some(3) {
                    tx_val
                        .get("blobVersionedHashes")
                        .and_then(|bv| bv.as_array())
                        .map(|arr| {
                            arr.iter().filter_map(|h| h.as_str()?.parse::<B256>().ok()).collect()
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                };

                tracing::info!("  -> {} blob(s) found", blob_hashes.len());

                let batch_tx =
                    BatchTransaction { block_number, block_hash, block_timestamp, blob_hashes };

                let _ = tx.send(Ok(L1Event::BatchTx(batch_tx))).await;
            }
        }
    }

    // Fetch logs for deposits
    let logs_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "eth_getLogs",
        "params": [{
            "fromBlock": format!("0x{:x}", block_number),
            "toBlock": format!("0x{:x}", block_number),
            "address": format!("{:?}", portal),
            "topics": [format!("{:?}", TRANSACTION_DEPOSITED_TOPIC)]
        }]
    });

    let logs_resp = client
        .post(&http_url)
        .json(&logs_req)
        .send()
        .await
        .map_err(|e| PipelineError::Http(e.to_string()))?;

    let logs_json: serde_json::Value =
        logs_resp.json().await.map_err(|e| PipelineError::Json(e.to_string()))?;

    if let Some(logs) = logs_json.get("result").and_then(|r| r.as_array()) {
        for (idx, log) in logs.iter().enumerate() {
            let topics: Vec<B256> = log
                .get("topics")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|t| t.as_str()?.parse::<B256>().ok()).collect())
                .unwrap_or_default();

            let data = log
                .get("data")
                .and_then(|d| d.as_str())
                .and_then(|s| hex::decode(s.trim_start_matches("0x")).ok())
                .unwrap_or_default();

            let log_index = log
                .get("logIndex")
                .and_then(|l| l.as_str())
                .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(idx as u64);

            match DepositedTransaction::from_log(&data, &topics, block_hash, log_index) {
                Ok(deposit) => {
                    let info = DepositInfo { l1_block_number: block_number, deposit };
                    let _ = tx.send(Ok(L1Event::Deposit(info))).await;
                }
                Err(e) => {
                    tracing::warn!("Failed to parse deposit log: {}", e);
                }
            }
        }
    }

    Ok(())
}
