/**
 * RSFGA Custom Metrics for k6
 *
 * Provides domain-specific metrics for authorization performance analysis.
 */

import { Rate, Trend, Counter, Gauge } from 'k6/metrics';

// ============ Check API Metrics ============

/** Latency trend for check operations */
export const checkLatency = new Trend('check_latency', true);

/** Rate of cache hits (estimated from response patterns) */
export const cacheHitRate = new Rate('cache_hit_rate');

/** Graph traversal depth observed in responses */
export const graphDepth = new Trend('graph_depth');

/** Check operations that returned allowed=true */
export const checkAllowedRate = new Rate('check_allowed_rate');

/** Count of check operations */
export const checkCount = new Counter('check_count');

// ============ Batch Check Metrics ============

/** Number of checks per batch request */
export const checksPerBatch = new Trend('checks_per_batch');

/** Rate of deduplicated checks within batches */
export const deduplicationRate = new Rate('dedup_rate');

/** Latency per individual check in batch */
export const batchCheckLatency = new Trend('batch_check_latency', true);

/** Count of batch check operations */
export const batchCheckCount = new Counter('batch_check_count');

// ============ Write Metrics ============

/** Latency for write operations */
export const writeLatency = new Trend('write_latency', true);

/** Count of tuples written */
export const tuplesWritten = new Counter('tuples_written');

/** Count of tuples deleted */
export const tuplesDeleted = new Counter('tuples_deleted');

/** Write throughput (tuples per second) */
export const writeThroughput = new Trend('write_throughput');

// ============ ListObjects Metrics ============

/** Latency for ListObjects operations */
export const listObjectsLatency = new Trend('list_objects_latency', true);

/** Number of objects returned per request */
export const objectsReturned = new Trend('objects_returned');

/** Count of ListObjects operations */
export const listObjectsCount = new Counter('list_objects_count');

// ============ ListUsers Metrics ============

/** Latency for ListUsers operations */
export const listUsersLatency = new Trend('list_users_latency', true);

/** Number of users returned per request */
export const usersReturned = new Trend('users_returned');

/** Count of ListUsers operations */
export const listUsersCount = new Counter('list_users_count');

// ============ Expand Metrics ============

/** Latency for Expand operations */
export const expandLatency = new Trend('expand_latency', true);

/** Count of Expand operations */
export const expandCount = new Counter('expand_count');

// ============ System Metrics ============

/** Current number of active connections (estimated) */
export const activeConnections = new Gauge('active_connections');

/** Error rate across all operations */
export const errorRate = new Rate('error_rate');

/** Rate of timeout errors */
export const timeoutRate = new Rate('timeout_rate');

/** Rate of depth limit exceeded errors */
export const depthLimitRate = new Rate('depth_limit_rate');

// ============ Metric Recording Functions ============

/**
 * Record check operation metrics
 */
export function recordCheck(response, allowed) {
  checkLatency.add(response.duration);
  checkCount.add(1);
  checkAllowedRate.add(allowed);
  errorRate.add(!response.success);

  // Estimate cache hit based on very fast responses (<2ms)
  cacheHitRate.add(response.duration < 2);
}

/**
 * Record batch check operation metrics
 */
export function recordBatchCheck(response, checksInBatch, resultsCount) {
  batchCheckLatency.add(response.duration / checksInBatch);
  batchCheckCount.add(1);
  checksPerBatch.add(checksInBatch);
  errorRate.add(!response.success);

  // Estimate deduplication rate
  if (checksInBatch > resultsCount) {
    deduplicationRate.add(true);
  } else {
    deduplicationRate.add(false);
  }
}

/**
 * Record write operation metrics
 */
export function recordWrite(response, writesCount, deletesCount) {
  writeLatency.add(response.duration);
  tuplesWritten.add(writesCount);
  tuplesDeleted.add(deletesCount);
  errorRate.add(!response.success);

  // Calculate throughput (tuples per second)
  if (response.duration > 0) {
    const totalOps = writesCount + deletesCount;
    const throughput = (totalOps / response.duration) * 1000;
    writeThroughput.add(throughput);
  }
}

/**
 * Record ListObjects operation metrics
 */
export function recordListObjects(response, objectCount) {
  listObjectsLatency.add(response.duration);
  listObjectsCount.add(1);
  objectsReturned.add(objectCount);
  errorRate.add(!response.success);
}

/**
 * Record ListUsers operation metrics
 */
export function recordListUsers(response, userCount) {
  listUsersLatency.add(response.duration);
  listUsersCount.add(1);
  usersReturned.add(userCount);
  errorRate.add(!response.success);
}

/**
 * Record Expand operation metrics
 */
export function recordExpand(response) {
  expandLatency.add(response.duration);
  expandCount.add(1);
  errorRate.add(!response.success);
}

/**
 * Record error types
 */
export function recordError(response) {
  errorRate.add(true);

  if (response.body && typeof response.body === 'object') {
    const code = response.body.code;
    if (code === 'depth_limit_exceeded') {
      depthLimitRate.add(true);
    }
  }

  // Check for timeout (status 0 or very long duration)
  if (response.status === 0 || response.duration > 30000) {
    timeoutRate.add(true);
  }
}

export default {
  checkLatency,
  cacheHitRate,
  graphDepth,
  checkAllowedRate,
  checkCount,
  checksPerBatch,
  deduplicationRate,
  batchCheckLatency,
  batchCheckCount,
  writeLatency,
  tuplesWritten,
  tuplesDeleted,
  writeThroughput,
  listObjectsLatency,
  objectsReturned,
  listObjectsCount,
  listUsersLatency,
  usersReturned,
  listUsersCount,
  expandLatency,
  expandCount,
  activeConnections,
  errorRate,
  timeoutRate,
  depthLimitRate,
  recordCheck,
  recordBatchCheck,
  recordWrite,
  recordListObjects,
  recordListUsers,
  recordExpand,
  recordError,
};
