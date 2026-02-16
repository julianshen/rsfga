use metrics::{counter, gauge, histogram};

pub fn record_event_received(event_type: &str) {
    counter!("precompute_events_received_total", "event_type" => event_type.to_string())
        .increment(1);
}

pub fn record_job_queued() {
    counter!("precompute_jobs_queued_total").increment(1);
}

pub fn record_job_completed(result: &str) {
    counter!("precompute_jobs_completed_total", "result" => result.to_string()).increment(1);
}

pub fn record_resolution_duration(duration_secs: f64) {
    histogram!("precompute_resolution_duration_seconds").record(duration_secs);
}

pub fn record_valkey_write_duration(duration_secs: f64) {
    histogram!("precompute_valkey_write_duration_seconds").record(duration_secs);
}

pub fn update_queue_depth(depth: f64) {
    gauge!("precompute_queue_depth").set(depth);
}
