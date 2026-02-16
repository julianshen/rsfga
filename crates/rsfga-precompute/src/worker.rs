//! Precomputation worker pool.
//!
//! Receives recomputation jobs, resolves checks using the graph resolver,
//! and writes results to Valkey.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use rsfga_domain::resolver::{CheckRequest, GraphResolver, ModelReader, TupleReader};
use rsfga_valkey::cache::{CheckCache, PrecomputedResult};
use rsfga_valkey::keys::CheckKey;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::impact::RecomputeJob;
use crate::metrics;

/// Trait for fetching the latest model ID for a store.
/// Implemented by DataStore adapters.
#[async_trait::async_trait]
pub trait ModelIdProvider: Send + Sync {
    async fn get_latest_model_id(&self, store_id: &str) -> Option<String>;
}

/// A worker pool that processes recomputation jobs concurrently.
pub struct WorkerPool {
    sender: mpsc::Sender<RecomputeJob>,
    _handles: Vec<tokio::task::JoinHandle<()>>,
}

impl WorkerPool {
    /// Create a new worker pool.
    pub fn new<T, M, P>(
        workers: usize,
        max_queue_depth: usize,
        resolver: Arc<GraphResolver<T, M>>,
        cache: Arc<CheckCache>,
        model_provider: Arc<P>,
    ) -> Self
    where
        T: TupleReader + 'static,
        M: ModelReader + 'static,
        P: ModelIdProvider + 'static,
    {
        let (sender, receiver) = mpsc::channel::<RecomputeJob>(max_queue_depth);
        let receiver = Arc::new(tokio::sync::Mutex::new(receiver));

        let mut handles = Vec::with_capacity(workers);

        for worker_id in 0..workers {
            let rx = Arc::clone(&receiver);
            let resolver = Arc::clone(&resolver);
            let cache = Arc::clone(&cache);
            let model_provider = Arc::clone(&model_provider);

            let handle = tokio::spawn(async move {
                loop {
                    let job = {
                        let mut guard = rx.lock().await;
                        guard.recv().await
                    };

                    match job {
                        Some(job) => {
                            process_job(worker_id, &job, &resolver, &cache, &*model_provider)
                                .await;
                        }
                        None => {
                            debug!(worker_id, "Worker channel closed, shutting down");
                            break;
                        }
                    }
                }
            });

            handles.push(handle);
        }

        Self {
            sender,
            _handles: handles,
        }
    }

    /// Submit a job for precomputation. Returns false if queue is full.
    pub fn submit(&self, job: RecomputeJob) -> bool {
        match self.sender.try_send(job) {
            Ok(_) => {
                metrics::record_job_queued();
                true
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!("Precompute job queue full, dropping job");
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                error!("Precompute worker pool shut down");
                false
            }
        }
    }

    /// Get the current queue depth.
    pub fn queue_depth(&self) -> usize {
        self.sender.max_capacity() - self.sender.capacity()
    }
}

async fn process_job<T, M, P>(
    worker_id: usize,
    job: &RecomputeJob,
    resolver: &GraphResolver<T, M>,
    cache: &CheckCache,
    model_provider: &P,
) where
    T: TupleReader + 'static,
    M: ModelReader + 'static,
    P: ModelIdProvider,
{
    let start = Instant::now();

    // Get the latest authorization model ID for this store
    let model_id = match model_provider.get_latest_model_id(&job.store_id).await {
        Some(id) => id,
        None => {
            debug!(
                worker_id,
                store_id = %job.store_id,
                "No authorization model found, skipping job"
            );
            metrics::record_job_completed("skipped");
            return;
        }
    };

    // Build check request
    let check_request = CheckRequest {
        store_id: job.store_id.clone(),
        user: job.user.clone(),
        relation: job.relation.clone(),
        object: format!("{}:{}", job.object_type, job.object_id),
        contextual_tuples: Arc::new(Vec::new()),
        context: Arc::new(HashMap::new()),
        authorization_model_id: None,
    };

    // Resolve the check
    let check_result = match resolver.check(&check_request).await {
        Ok(result) => result,
        Err(e) => {
            warn!(
                worker_id,
                store_id = %job.store_id,
                error = %e,
                "Check resolution failed"
            );
            metrics::record_job_completed("error");
            return;
        }
    };

    let resolution_duration = start.elapsed();
    metrics::record_resolution_duration(resolution_duration.as_secs_f64());

    // Write result to Valkey
    let valkey_start = Instant::now();
    let key = CheckKey::new(
        &job.store_id,
        &model_id,
        &job.object_type,
        &job.object_id,
        &job.relation,
        &job.user,
    );
    let precomputed = PrecomputedResult {
        allowed: check_result.allowed,
        computed_at: chrono::Utc::now(),
    };

    if let Err(e) = cache.set(&key, &precomputed).await {
        warn!(
            worker_id,
            key = %key.to_redis_key(),
            error = %e,
            "Failed to write precomputed result to Valkey"
        );
        metrics::record_job_completed("error");
        return;
    }

    metrics::record_valkey_write_duration(valkey_start.elapsed().as_secs_f64());
    metrics::record_job_completed("success");

    debug!(
        worker_id,
        key = %key.to_redis_key(),
        allowed = check_result.allowed,
        resolution_ms = resolution_duration.as_millis(),
        "Precomputed check result"
    );
}
