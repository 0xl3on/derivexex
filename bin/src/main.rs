use std::sync::Arc;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use clap::Parser;
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

use derivexex_stream::UnichainPipeline;

#[derive(Parser)]
#[command(name = "derivexex-streamer")]
#[command(about = "WebSocket server for streaming Unichain derivation events")]
struct Args {
    /// L1 WebSocket URL
    #[arg(long, env = "L1_WS_URL")]
    l1_ws_url: String,

    /// Beacon API URL
    #[arg(long, env = "BEACON_URL")]
    beacon_url: String,

    /// WebSocket server port
    #[arg(long, default_value = "9944")]
    port: u16,

    /// Blob cache capacity
    #[arg(long, default_value = "128")]
    blob_cache: u32,
}

#[derive(Clone)]
struct AppState {
    event_tx: Arc<broadcast::Sender<String>>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("info,derivexex_streamer=debug,derivexex_pipeline=debug")
        }))
        .init();

    let args = Args::parse();

    info!("Starting Unichain streaming pipeline...");
    info!("L1 WS: {}", &args.l1_ws_url);
    info!("Beacon: {}", &args.beacon_url);
    info!("Port: {}", args.port);

    // Create the pipeline
    let pipeline = UnichainPipeline::builder()
        .l1_ws_url(args.l1_ws_url.clone())
        .beacon_url(args.beacon_url.clone())
        .blob_cache_capacity(args.blob_cache)
        .build()
        .start()
        .await?;

    // Create broadcast channel for WebSocket clients
    let (event_tx, _) = broadcast::channel::<String>(1024);
    let event_tx = Arc::new(event_tx);

    // Spawn pipeline block event forwarder
    let tx_clone = event_tx.clone();
    let mut event_stream = pipeline.stream_l2_blocks();
    tokio::spawn(async move {
        info!("[BLOCK_FORWARDER] Task started");
        while let Some(result) = event_stream.next().await {
            match result {
                Ok(event) => {
                    // Log what type of event we received
                    let event_type = match &event {
                        derivexex_stream::PipelineEvent::Block(b) => {
                            format!(
                                "Block(timestamp={}, txs={})",
                                b.timestamp,
                                b.transactions.len()
                            )
                        }
                        derivexex_stream::PipelineEvent::Reorg(r) => {
                            format!("Reorg(depth={})", r.depth)
                        }
                        derivexex_stream::PipelineEvent::HeadUpdate(h) => {
                            format!("HeadUpdate(unsafe={}, safe={})", h.unsafe_head, h.safe_head)
                        }
                    };
                    debug!("[BLOCK_FORWARDER] Received event: {}", event_type);

                    match serde_json::to_string(&event) {
                        Ok(json) => {
                            let json_len = json.len();
                            let receivers = tx_clone.receiver_count();
                            debug!(
                                "[BLOCK_FORWARDER] Serialized {} -> {} bytes, {} receivers",
                                event_type, json_len, receivers
                            );
                            match tx_clone.send(json) {
                                Ok(n) => {
                                    debug!("[BLOCK_FORWARDER] Sent to {} receivers", n);
                                }
                                Err(e) => {
                                    warn!("[BLOCK_FORWARDER] Send failed (no receivers?): {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            error!("[BLOCK_FORWARDER] Failed to serialize {}: {}", event_type, e);
                        }
                    }
                }
                Err(e) => {
                    warn!("[BLOCK_FORWARDER] Pipeline event error: {}", e);
                }
            }
        }
        warn!("[BLOCK_FORWARDER] Task ended - stream closed");
    });

    // Spawn pipeline transaction forwarder
    let tx_clone = event_tx.clone();
    let mut tx_stream = pipeline.stream_transactions();
    info!("[TX_FORWARDER] Task started");
    tokio::spawn(async move {
        while let Some(result) = tx_stream.next().await {
            match result {
                Ok(decoded_tx) => {
                    debug!("[TX_FORWARDER] Decoded tx: {} -> {:?}", decoded_tx.hash, decoded_tx.to);
                    // Wrap in a type field for the client
                    let event = serde_json::json!({
                        "type": "transaction",
                        "data": decoded_tx
                    });
                    match serde_json::to_string(&event) {
                        Ok(json) => {
                            let json_len = json.len();
                            let receivers = tx_clone.receiver_count();
                            debug!(
                                "[TX_FORWARDER] Serialized tx {} -> {} bytes, {} receivers",
                                decoded_tx.hash, json_len, receivers
                            );
                            match tx_clone.send(json) {
                                Ok(n) => {
                                    debug!("[TX_FORWARDER] Sent to {} receivers", n);
                                }
                                Err(e) => {
                                    warn!("[TX_FORWARDER] Send failed: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            error!("[TX_FORWARDER] Failed to serialize tx: {}", e);
                        }
                    }
                }
                Err(e) => {
                    warn!("[TX_FORWARDER] Transaction decode error: {}", e);
                }
            }
        }
        warn!("[TX_FORWARDER] Task ended - stream closed");
    });

    // Spawn pipeline runner
    let pipeline = Arc::new(pipeline);
    let pipeline_clone = pipeline.clone();
    tokio::spawn(async move {
        if let Err(e) = pipeline_clone.run().await {
            error!("Pipeline error: {}", e);
        }
    });

    // Build axum router
    let state = AppState { event_tx };

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(health_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    // Start server
    let addr = format!("0.0.0.0:{}", args.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("WebSocket server listening on ws://{}/ws", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

async fn health_handler() -> &'static str {
    "ok"
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    info!("[WS] New client connected");

    // Send welcome message
    let welcome = serde_json::json!({
        "type": "connected",
        "message": "Connected to Unichain streaming pipeline"
    });
    if sender.send(Message::Text(welcome.to_string().into())).await.is_err() {
        warn!("[WS] Failed to send welcome message");
        return;
    }

    let mut rx = state.event_tx.subscribe();
    info!("[WS] Client subscribed to event channel");

    loop {
        tokio::select! {
            // Forward pipeline events to client
            result = rx.recv() => {
                match result {
                    Ok(msg) => {
                        // Extract event type from JSON for logging
                        let event_type = serde_json::from_str::<serde_json::Value>(&msg)
                            .ok()
                            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from))
                            .unwrap_or_else(|| "unknown".to_string());

                        debug!("[WS] Forwarding event: {} ({} bytes)", event_type, msg.len());

                        if sender.send(Message::Text(msg.into())).await.is_err() {
                            warn!("[WS] Failed to send to client, disconnecting");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("[WS] Client lagged, missed {} messages", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        warn!("[WS] Event channel closed");
                        break;
                    }
                }
            }
            // Handle incoming messages (ping/pong, close)
            Some(msg) = receiver.next() => {
                match msg {
                    Ok(Message::Close(_)) => {
                        info!("[WS] Client requested close");
                        break;
                    }
                    Ok(Message::Ping(data)) => {
                        if sender.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("[WS] Client error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    info!("[WS] Client disconnected");
}
