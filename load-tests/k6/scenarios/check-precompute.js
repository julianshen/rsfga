/**
 * Check API - Precompute Cache Load Test
 *
 * Compares check latency and throughput with and without precompute cache.
 * Three-stage test:
 *   1. Warm-up: populate the hot-path registry via check requests.
 *   2. Trigger: write a dummy tuple to generate a NATS committed event,
 *      causing the precompute worker to re-scan the now-populated hot-path
 *      and fill the Valkey cache.
 *   3. Measure: constant-arrival-rate check workload against the warm cache.
 *
 * Expected results with precompute enabled + warm cache:
 *   - >50% cache hit rate (responses <1ms; conservative for random workloads)
 *   - p95 latency <5ms
 *   - Throughput improvement vs cold baseline
 *
 * Usage:
 *   # Against RSFGA with precompute enabled + warm Valkey
 *   k6 run -e RSFGA_URL=http://localhost:8080 check-precompute.js
 *
 *   # With custom warm-up wait (seconds for precompute worker to fill cache)
 *   k6 run -e RSFGA_URL=http://localhost:8080 -e WARMUP_WAIT=15 check-precompute.js
 */

import { sleep } from 'k6';
import { createClient } from '../lib/client.js';
import { TestSetup, uniqueStoreName, randomUser, randomObject } from '../lib/setup.js';
import { recordCheck, errorRate } from '../lib/metrics.js';
import { Rate, Trend } from 'k6/metrics';

// Configuration from environment or defaults
const BASE_URL = __ENV.RSFGA_URL || 'http://localhost:8080';
const USER_COUNT = parseInt(__ENV.USER_COUNT) || 500;
const OBJECT_COUNT = parseInt(__ENV.OBJECT_COUNT) || 50;
const WARMUP_WAIT = parseInt(__ENV.WARMUP_WAIT) || 10;

// Timing constants for the three-stage pipeline.
// The warmup maxDuration is 2m; the trigger fires after warmup completes to
// generate a NATS committed event that causes the precompute worker to scan
// the hot-path and populate the cache before measurement begins.
const WARMUP_MAX_DURATION_S = 120;
const PRECOMPUTE_WORKER_DELAY_S = 30;
const TRIGGER_START_TIME_S = WARMUP_MAX_DURATION_S + 5;
const MEASURE_START_TIME_S = TRIGGER_START_TIME_S + PRECOMPUTE_WORKER_DELAY_S;

// Precompute-specific metrics
const precomputeHitRate = new Rate('precompute_hit_rate');
const precomputeLatency = new Trend('precompute_latency', true);

// Test options: warm-up then sustained measurement
export const options = {
  scenarios: {
    // Stage 1: Warm-up — send checks to populate hot-path registry
    warmup: {
      executor: 'shared-iterations',
      vus: 10,
      iterations: USER_COUNT * 2,
      maxDuration: `${WARMUP_MAX_DURATION_S}s`,
      exec: 'warmup',
    },
    // Stage 2: Trigger — write a dummy tuple to generate a NATS committed
    // event, causing the precompute worker to re-scan the hot-path.
    trigger: {
      executor: 'shared-iterations',
      vus: 1,
      iterations: 1,
      maxDuration: '30s',
      startTime: `${TRIGGER_START_TIME_S}s`,
      exec: 'triggerPrecompute',
    },
    // Stage 3: Measurement — sustained load against warm cache.
    measure: {
      executor: 'constant-arrival-rate',
      rate: 200,
      timeUnit: '1s',
      duration: '3m',
      preAllocatedVUs: 50,
      maxVUs: 200,
      startTime: `${MEASURE_START_TIME_S}s`,
      exec: 'measure',
    },
  },
  thresholds: {
    'http_req_duration{endpoint:check}': ['p(95)<10', 'p(99)<30'],
    'http_req_failed{endpoint:check}': ['rate<0.001'],
    'check_latency': ['p(95)<10', 'p(99)<30'],
    'precompute_latency': ['p(95)<5'],
    'precompute_hit_rate': ['rate>0.5'],
    'error_rate': ['rate<0.001'],
  },
};

// Shared state
let warmupClient = null;
let measureClient = null;

// Simple model for direct checks
const simpleModel = {
  schema_version: '1.1',
  type_definitions: [
    { type: 'user' },
    {
      type: 'document',
      relations: {
        viewer: { this: {} },
        editor: { this: {} },
      },
      metadata: {
        relations: {
          viewer: { directly_related_user_types: [{ type: 'user' }] },
          editor: { directly_related_user_types: [{ type: 'user' }] },
        },
      },
    },
  ],
};

/**
 * Setup function — create store, model, and tuples.
 */
export function setup() {
  const setupClient = new TestSetup(BASE_URL);

  const storeName = uniqueStoreName('check-precompute');
  const storeId = setupClient.createStore(storeName);
  console.log(`Created store: ${storeId}`);

  const modelId = setupClient.writeModel(simpleModel);
  console.log(`Created model: ${modelId}`);

  // Generate tuples: each user is viewer of ~10% of objects
  const tuples = [];
  const relations = ['viewer', 'editor'];

  for (let u = 0; u < USER_COUNT; u++) {
    const objectsPerUser = Math.max(1, Math.floor(OBJECT_COUNT * 0.1));
    for (let i = 0; i < objectsPerUser; i++) {
      tuples.push({
        user: `user:user_${u}`,
        relation: relations[i % relations.length],
        object: `document:doc_${(u * objectsPerUser + i) % OBJECT_COUNT}`,
      });
    }
  }

  const written = setupClient.writeTuples(tuples);
  console.log(`Wrote ${written} tuples`);

  return {
    storeId,
    modelId,
    userCount: USER_COUNT,
    objectCount: OBJECT_COUNT,
    warmupWait: WARMUP_WAIT,
  };
}

/**
 * Warm-up phase — send diverse checks to populate the hot-path registry.
 * The precompute worker will pick these up and populate the Valkey cache.
 */
export function warmup(data) {
  if (!warmupClient) {
    warmupClient = createClient(BASE_URL);
  }

  const { storeId, modelId, userCount, objectCount } = data;

  const user = randomUser(userCount);
  const object = randomObject('document', objectCount);
  const relation = Math.random() > 0.5 ? 'viewer' : 'editor';

  const res = warmupClient.check(storeId, user, relation, object, null, modelId);

  // Record but don't enforce strict thresholds during warm-up
  if (res.success) {
    recordCheck(res, res.body && res.body.allowed === true);
  }
}

/**
 * Trigger phase — write a dummy tuple to generate a NATS committed event.
 * This causes the precompute worker to re-scan the now-populated hot-path
 * registry and fill the Valkey cache before the measurement phase begins.
 */
export function triggerPrecompute(data) {
  const client = createClient(BASE_URL);
  const { storeId, modelId } = data;

  // Write a dummy tuple to generate a NATS committed event.
  const writes = [{
    user: 'user:_trigger',
    relation: 'viewer',
    object: 'document:doc_trigger',
  }];
  const res = client.write(storeId, writes, [], modelId);

  if (res.success) {
    console.log(`Trigger write succeeded — precompute worker should re-scan hot-path`);
  } else {
    console.log(`Trigger write failed: ${JSON.stringify(res.body)}`);
  }

  // Wait for precompute worker to process the event and fill the cache
  sleep(PRECOMPUTE_WORKER_DELAY_S);
}

/**
 * Measurement phase — sustained check workload against warm precompute cache.
 */
export function measure(data) {
  if (!measureClient) {
    measureClient = createClient(BASE_URL);
  }

  const { storeId, modelId, userCount, objectCount } = data;

  const user = randomUser(userCount);
  const object = randomObject('document', objectCount);
  const relation = Math.random() > 0.5 ? 'viewer' : 'editor';

  const res = measureClient.check(storeId, user, relation, object, null, modelId);

  if (res.success) {
    const allowed = res.body && res.body.allowed === true;
    recordCheck(res, allowed);

    // Track precompute-specific metrics
    precomputeLatency.add(res.duration);

    // Estimate cache hit: sub-millisecond responses likely served from Valkey
    precomputeHitRate.add(res.duration < 1);
  }

  // Small jitter to avoid thundering herd
  sleep(Math.random() * 0.05);
}

/**
 * Teardown — clean up the store.
 */
export function teardown(data) {
  const teardownClient = createClient(BASE_URL);
  teardownClient.deleteStore(data.storeId);
  console.log(`Deleted store: ${data.storeId}`);
}
