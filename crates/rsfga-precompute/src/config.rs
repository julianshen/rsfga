use std::time::Duration;

#[derive(Debug, Clone)]
pub struct PrecomputeConfig {
    pub consumer_name: String,
    pub workers: usize,
    pub batch_window: Duration,
    pub max_queue_depth: usize,
    pub fetch_timeout: Duration,
}

impl Default for PrecomputeConfig {
    fn default() -> Self {
        Self {
            consumer_name: "rsfga-precompute-1".to_string(),
            workers: 4,
            batch_window: Duration::from_millis(50),
            max_queue_depth: 10_000,
            fetch_timeout: Duration::from_millis(500),
        }
    }
}
