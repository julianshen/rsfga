/**
 * Precompute Cache Scalability Test
 *
 * Tests precompute behavior at scale with configurable entry counts.
 * Validates that latency and hit rate remain acceptable as the number
 * of hot-path entries grows.
 *
 * Parameterized via environment variables:
 *   USER_COUNT  — number of users (default 500)
 *   OBJECT_COUNT — number of objects (default 200)
 *
 * Example configurations:
 *   ~10K entries:   USER_COUNT=1000 OBJECT_COUNT=10
 *   ~100K entries:  USER_COUNT=5000 OBJECT_COUNT=20
 *   Small baseline: USER_COUNT=100  OBJECT_COUNT=10
 *
 * Three-stage pipeline:
 *   1. Warmup: populate hot-path registry via exhaustive checks.
 *   2. Trigger: dummy write to fire precompute worker.
 *   3. Measure: constant-arrival-rate (200 req/s) for 3 minutes.
 *
 * Usage:
 *   k6 run -e RSFGA_URL=http://localhost:8080 precompute-scale.js
 *   k6 run -e RSFGA_URL=http://localhost:8080 -e USER_COUNT=5000 -e OBJECT_COUNT=20 precompute-scale.js
 */

import { sleep } from 'k6';
import { createClient } from '../lib/client.js';
import { TestSetup, uniqueStoreName, randomUser, randomObject } from '../lib/setup.js';
import { recordCheck, errorRate } from '../lib/metrics.js';
import { Rate, Trend, Counter } from 'k6/metrics';

const BASE_URL = __ENV.RSFGA_URL || 'http://localhost:8080';
const USER_COUNT = parseInt(__ENV.USER_COUNT) || 500;
const OBJECT_COUNT = parseInt(__ENV.OBJECT_COUNT) || 200;

// Hit-rate proxy threshold (ms). This is a latency-based *approximation*,
// not an actual cache hit counter. Responses faster than this are likely
// served from Valkey rather than the graph resolver, but this can
// misclassify slow cache hits and fast misses under load.
const HIT_THRESHOLD_MS = 5;

// Warmup iterations: cover as much of the combinatorial space as possible.
// Cap at 5000 to keep warmup time reasonable.
const WARMUP_ITERATIONS = Math.min(USER_COUNT * 2, 5000);

// Timing constants
const WARMUP_MAX_DURATION_S = 180;
const PRECOMPUTE_WORKER_DELAY_S = 30;
const TRIGGER_START_TIME_S = WARMUP_MAX_DURATION_S + 5;
const MEASURE_START_TIME_S = TRIGGER_START_TIME_S + PRECOMPUTE_WORKER_DELAY_S;

// Scale-specific metrics
const scaleHitRate = new Rate('scale_hit_rate');
const scaleLatency = new Trend('scale_latency', true);
const scaleWarmupRequests = new Counter('scale_warmup_requests');

export const options = {
  scenarios: {
    warmup: {
      executor: 'shared-iterations',
      vus: 20,
      iterations: WARMUP_ITERATIONS,
      maxDuration: `${WARMUP_MAX_DURATION_S}s`,
      exec: 'warmup',
    },
    trigger: {
      executor: 'shared-iterations',
      vus: 1,
      iterations: 1,
      maxDuration: '60s',
      startTime: `${TRIGGER_START_TIME_S}s`,
      exec: 'triggerPrecompute',
    },
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
    'http_req_duration{endpoint:check}': ['p(95)<20', 'p(99)<50'],
    'http_req_failed{endpoint:check}': ['rate<0.001'],
    'scale_latency': ['p(50)<5', 'p(95)<15', 'p(99)<30'],
    'error_rate': ['rate<0.001'],
  },
};

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

let warmupClient = null;
let measureClient = null;

export function setup() {
  const setupClient = new TestSetup(BASE_URL);
  const storeName = uniqueStoreName('precompute-scale');
  const storeId = setupClient.createStore(storeName);
  console.log(`Created store: ${storeId}`);
  console.log(`Scale parameters: USER_COUNT=${USER_COUNT}, OBJECT_COUNT=${OBJECT_COUNT}`);
  console.log(`Estimated combinatorial space: ${USER_COUNT * OBJECT_COUNT * 2} check combos`);

  const modelId = setupClient.writeModel(simpleModel);
  console.log(`Created model: ${modelId}`);

  // Generate tuples: each user is viewer/editor of a subset of objects
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
  console.log(`Warmup iterations: ${WARMUP_ITERATIONS}`);

  return {
    storeId,
    modelId,
    userCount: USER_COUNT,
    objectCount: OBJECT_COUNT,
  };
}

/**
 * Warmup — populate hot-path registry with diverse checks.
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
  if (res.success) {
    scaleWarmupRequests.add(1);
    recordCheck(res, res.body && res.body.allowed === true);
  } else {
    errorRate.add(true);
  }
}

/**
 * Trigger — dummy write to fire precompute worker.
 */
export function triggerPrecompute(data) {
  const client = createClient(BASE_URL);
  const { storeId, modelId } = data;

  const writes = [{
    user: 'user:_trigger',
    relation: 'viewer',
    object: 'document:doc_trigger',
  }];
  const res = client.write(storeId, writes, [], modelId);

  if (res.success) {
    console.log('Trigger write succeeded — precompute worker should re-scan hot-path');
  } else {
    console.log(`Trigger write failed: ${JSON.stringify(res.body)}`);
  }

  // Wait for precompute worker to process. The measure scenario's startTime
  // already accounts for this delay, so this sleep just keeps the trigger
  // VU alive until the worker has had time to act.
  sleep(PRECOMPUTE_WORKER_DELAY_S);
}

/**
 * Measure — sustained check workload against the warm precompute cache.
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
    scaleLatency.add(res.duration);
    scaleHitRate.add(res.duration < HIT_THRESHOLD_MS);
  } else {
    errorRate.add(true);
  }

  sleep(Math.random() * 0.05);
}

export function teardown(data) {
  const teardownClient = createClient(BASE_URL);
  teardownClient.deleteStore(data.storeId);
  console.log(`Deleted store: ${data.storeId}`);
}
