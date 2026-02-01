//! Main pipeline orchestrator for Unichain streaming derivation.

use std::{pin::Pin, sync::Arc};

use alloy_primitives::{B256, U256};
use bon::Builder;
use futures::{stream::Stream, StreamExt};
use tokio::sync::{broadcast, mpsc, RwLock};

use derivexex_pipeline::{
    config::{
        BASE_FEE_SCALAR, BATCHER, BLOB_BASE_FEE_SCALAR, DEFAULT_BEACON_URL, DEFAULT_L1_WS_URL,
        L2_BLOCK_TIME, L2_GENESIS_TIME,
    },
    decode_blob_data,
    l1_info::Hardfork,
    ChannelAssembler, Deriver, DeriverConfig, EpochInfo, FrameDecoder, L1BlockRef, L2Block,
};

use super::{
    blob_fetcher::BlobFetcher,
    heads::{HeadTracker, HeadUpdate},
    l1_fetcher::L1Fetcher,
    l1_follower::{L1Event, L1Follower},
    reorg::{ReorgDetector, ReorgEvent},
    tx_decoder::{decode_deposit_tx, decode_transaction, DecodedTransaction},
    PipelineError,
};

/// Events emitted by the pipeline.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipelineEvent {
    /// New L2 block derived.
    Block(L2Block),
    /// L1 reorg detected; L2 state may need to be rolled back.
    Reorg(ReorgEvent),
    /// Head positions updated.
    HeadUpdate(HeadUpdate),
}

/// Pipeline state shared between components.
struct PipelineState {
    deriver: Deriver,
    assembler: ChannelAssembler,
    reorg_detector: ReorgDetector,
    head_tracker: HeadTracker,
}

/// Builder for UnichainPipeline.
#[derive(Builder)]
pub struct UnichainPipelineConfig {
    /// L1 WebSocket URL (defaults to public endpoint).
    #[builder(default = DEFAULT_L1_WS_URL.to_string())]
    l1_ws_url: String,

    /// Beacon API URL (defaults to public endpoint).
    #[builder(default = DEFAULT_BEACON_URL.to_string())]
    beacon_url: String,

    /// Blob cache capacity.
    #[builder(default = 128)]
    blob_cache_capacity: u32,

    /// Reorg detection depth.
    #[builder(default = 64)]
    reorg_depth: usize,
}

/// Streaming pipeline for Unichain L2 block derivation.
pub struct UnichainPipeline {
    config: UnichainPipelineConfig,
    state: Arc<RwLock<PipelineState>>,
    blob_fetcher: Arc<BlobFetcher>,
    l1_fetcher: Arc<L1Fetcher>,
    event_tx: broadcast::Sender<PipelineEvent>,
}

impl UnichainPipeline {
    /// Create a pipeline builder.
    pub fn builder() -> UnichainPipelineConfigBuilder {
        UnichainPipelineConfig::builder()
    }

    /// Build a new pipeline from config.
    pub async fn from_config(config: UnichainPipelineConfig) -> Result<Self, PipelineError> {
        let deriver_config = DeriverConfig::builder()
            .hardfork(Hardfork::Isthmus)
            .batcher_addr(BATCHER)
            .base_fee_scalar(BASE_FEE_SCALAR)
            .blob_base_fee_scalar(BLOB_BASE_FEE_SCALAR)
            .l2_genesis_time(L2_GENESIS_TIME)
            .l2_block_time(L2_BLOCK_TIME)
            .build();

        let state = PipelineState {
            deriver: Deriver::new(deriver_config),
            assembler: ChannelAssembler::new(),
            reorg_detector: ReorgDetector::new(config.reorg_depth),
            head_tracker: HeadTracker::mainnet(),
        };

        let blob_fetcher =
            Arc::new(BlobFetcher::new(&config.beacon_url, config.blob_cache_capacity));

        let l1_fetcher = Arc::new(L1Fetcher::new(&config.l1_ws_url));

        let (event_tx, _) = broadcast::channel(1024);

        Ok(Self { config, state: Arc::new(RwLock::new(state)), blob_fetcher, l1_fetcher, event_tx })
    }

    /// Stream L2 blocks as they are derived.
    pub fn stream_l2_blocks(
        &self,
    ) -> Pin<Box<dyn Stream<Item = Result<PipelineEvent, PipelineError>> + Send>> {
        let mut rx = self.event_tx.subscribe();
        Box::pin(async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(event) => yield Ok(event),
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        })
    }

    /// Stream decoded transactions from derived L2 blocks.
    pub fn stream_transactions(
        &self,
    ) -> Pin<Box<dyn Stream<Item = Result<DecodedTransaction, PipelineError>> + Send>> {
        let mut rx = self.event_tx.subscribe();
        Box::pin(async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(PipelineEvent::Block(block)) => {
                        for tx in &block.transactions {
                            match tx {
                                derivexex_pipeline::L2Transaction::Deposit(deposit) => {
                                    let decoded = decode_deposit_tx(
                                        deposit,
                                        block.timestamp,
                                        block.l1_origin.number,
                                    );
                                    yield Ok(decoded);
                                }
                                derivexex_pipeline::L2Transaction::Sequencer(raw) => {
                                    match decode_transaction(
                                        raw.as_ref(),
                                        block.timestamp,
                                        block.l1_origin.number,
                                    ) {
                                        Ok(decoded) => yield Ok(decoded),
                                        Err(e) => {
                                            tracing::debug!(
                                                "Skipping malformed tx at timestamp {}: {} | raw: 0x{}",
                                                block.timestamp,
                                                e,
                                                hex::encode(&raw[..raw.len().min(100)])
                                            );
                                            // Skip malformed transactions instead of yielding error
                                        },
                                    }
                                }
                            }
                        }
                    }
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        })
    }

    /// Run the pipeline main loop.
    ///
    /// This connects to L1 via WebSocket, follows new blocks, fetches blobs,
    /// and derives L2 blocks.
    pub async fn run(&self) -> Result<(), PipelineError> {
        let l1_follower = L1Follower::new(&self.config.l1_ws_url);
        let mut l1_events = l1_follower.subscribe().await?;

        // Channel for internal processing
        let (process_tx, mut process_rx) = mpsc::channel::<ProcessTask>(256);

        // Spawn blob fetch and process tasks
        let blob_fetcher = self.blob_fetcher.clone();
        let l1_fetcher = self.l1_fetcher.clone();
        let state = self.state.clone();
        let event_tx = self.event_tx.clone();

        tokio::spawn(async move {
            while let Some(task) = process_rx.recv().await {
                match task {
                    ProcessTask::FetchAndProcessBlobs {
                        slot,
                        blob_hashes,
                        l1_ref,
                        basefee,
                        blob_basefee,
                    } => {
                        if let Err(e) = process_blobs(
                            &blob_fetcher,
                            &l1_fetcher,
                            &state,
                            &event_tx,
                            slot,
                            blob_hashes,
                            l1_ref,
                            basefee,
                            blob_basefee,
                        )
                        .await
                        {
                            tracing::error!("Failed to process blobs: {}", e);
                        }
                    }
                }
            }
        });

        // Main event loop
        while let Some(event) = l1_events.next().await {
            match event? {
                L1Event::NewBlock(header) => {
                    let block_number = header.block_number();
                    let block_hash = header.hash;
                    let timestamp: u64 = header.timestamp.try_into().unwrap_or(0);
                    tracing::debug!("L1 block {} ({})", block_number, block_hash);
                    let parent_hash = header.parent_hash;

                    // Check for reorg
                    let mut state = self.state.write().await;
                    if let Some(reorg) =
                        state.reorg_detector.process_block(block_number, block_hash, parent_hash)
                    {
                        tracing::warn!(
                            "L1 reorg detected at block {}: depth={}",
                            reorg.first_invalid,
                            reorg.depth
                        );

                        // Prune affected state
                        state.assembler.remove_frames_from(reorg.first_invalid);
                        state.deriver.clear_epochs_from(reorg.first_invalid);
                        state.head_tracker.handle_reorg(reorg.first_invalid);

                        let _ = self.event_tx.send(PipelineEvent::Reorg(reorg));
                    }

                    // Register epoch info for this block
                    let basefee = header.base_fee_per_gas.unwrap_or(U256::ZERO);
                    let blob_basefee = header.excess_blob_gas.unwrap_or(U256::ZERO);
                    state.deriver.register_epoch(
                        block_number,
                        EpochInfo {
                            l1_ref: L1BlockRef {
                                number: block_number,
                                hash: block_hash,
                                timestamp,
                            },
                            basefee,
                            blob_basefee,
                        },
                    );

                    // Update head tracker
                    let heads = state.head_tracker.update_l1_head(block_number);
                    let _ = self.event_tx.send(PipelineEvent::HeadUpdate(heads));
                }

                L1Event::BatchTx(batch) => {
                    if batch.blob_hashes.is_empty() {
                        continue;
                    }

                    // Compute slot from block timestamp
                    // Beacon chain genesis: Dec 1, 2020 12:00:23 PM UTC
                    const BEACON_GENESIS_TIME: u64 = 1606824023;
                    const SECONDS_PER_SLOT: u64 = 12;
                    let slot = batch.block_timestamp.saturating_sub(BEACON_GENESIS_TIME) /
                        SECONDS_PER_SLOT;

                    tracing::debug!(
                        "Batch in L1 block {} (timestamp={}, slot={})",
                        batch.block_number,
                        batch.block_timestamp,
                        slot
                    );

                    // Get L1 block info for epoch registration
                    let l1_ref = L1BlockRef {
                        number: batch.block_number,
                        hash: batch.block_hash,
                        timestamp: batch.block_timestamp,
                    };

                    let _ = process_tx
                        .send(ProcessTask::FetchAndProcessBlobs {
                            slot,
                            blob_hashes: batch.blob_hashes,
                            l1_ref,
                            basefee: U256::ZERO,
                            blob_basefee: U256::ZERO,
                        })
                        .await;
                }

                L1Event::Deposit(info) => {
                    let mut state = self.state.write().await;
                    state.deriver.add_deposit(info.l1_block_number, info.deposit);
                }
            }
        }

        Err(PipelineError::StreamEnded)
    }

    /// Get the current head positions.
    pub async fn heads(&self) -> HeadUpdate {
        self.state.read().await.head_tracker.heads()
    }
}

enum ProcessTask {
    FetchAndProcessBlobs {
        slot: u64,
        blob_hashes: Vec<B256>,
        l1_ref: L1BlockRef,
        basefee: U256,
        blob_basefee: U256,
    },
}

async fn process_blobs(
    blob_fetcher: &BlobFetcher,
    l1_fetcher: &L1Fetcher,
    state_lock: &RwLock<PipelineState>,
    event_tx: &broadcast::Sender<PipelineEvent>,
    slot: u64,
    blob_hashes: Vec<B256>,
    l1_ref: L1BlockRef,
    basefee: U256,
    blob_basefee: U256,
) -> Result<(), PipelineError> {
    // Fetch blobs
    let mut blobs = Vec::with_capacity(blob_hashes.len());
    for hash in blob_hashes {
        let blob = blob_fetcher.get_blob(slot, hash).await?;
        blobs.push(blob);
    }

    // Decode blobs into frames and get channels
    let channels = {
        let mut state = state_lock.write().await;

        // Register epoch info for the batch's containing block
        state.deriver.register_epoch(
            l1_ref.number,
            EpochInfo { l1_ref: l1_ref.clone(), basefee, blob_basefee },
        );

        for blob in &blobs {
            let data = decode_blob_data(blob);
            let frames = FrameDecoder::decode_frames(&data, l1_ref.number)?;
            for frame in frames {
                state.assembler.add_frame(frame);
            }
        }

        state.assembler.take_complete()
    };

    // Process complete channels, fetching missing epochs on demand
    for channel in channels {
        loop {
            let result = {
                let mut state = state_lock.write().await;
                state.deriver.process_channel(&channel)
            };

            match result {
                Ok(result) => {
                    let mut state = state_lock.write().await;
                    for block in result.blocks {
                        // l1_origin: the L1 block the L2 block references
                        // l1_inclusion: the L1 block where the batch was posted (l1_ref)
                        state.head_tracker.add_l2_block(
                            block.timestamp,
                            block.l1_origin.number,
                            l1_ref.number,
                        );
                        let _ = event_tx.send(PipelineEvent::Block(block));
                    }
                    break;
                }
                Err(derivexex_pipeline::DeriveError::MissingEpoch(missing_block)) => {
                    tracing::info!("Fetching missing epoch info for L1 block {}", missing_block);
                    match l1_fetcher.get_block_info(missing_block).await {
                        Ok(epoch_info) => {
                            let mut state = state_lock.write().await;
                            state.deriver.register_epoch(missing_block, epoch_info);
                            continue;
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to fetch epoch info for block {}: {}",
                                missing_block,
                                e
                            );
                            break;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to process channel: {}", e);
                    break;
                }
            }
        }
    }

    Ok(())
}

impl UnichainPipelineConfig {
    /// Start the pipeline with this configuration.
    pub async fn start(self) -> Result<UnichainPipeline, PipelineError> {
        UnichainPipeline::from_config(self).await
    }
}
