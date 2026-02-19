/**
 * Mixed Workload with Precompute Cache Load Test
 *
 * Validates that precomputation does not degrade write throughput (>5%)
 * while measuring check hit rate under concurrent writes.
 *
 * Three concurrent scenarios:
 *   1. Warmup: 1000 checks to populate the hot-path registry, then trigger
 *      a dummy write so the precompute worker fills the Valkey cache.
 *   2. Writes: 50 write req/s for 5 minutes (measure throughput with
 *      precompute overhead).
 *   3. Checks: 200 check req/s for 5 minutes (measure hit rate under
 *      concurrent writes).
 *
 * Usage:
 *   # Baseline (no precompute — start only RSFGA + PostgreSQL, no Valkey)
 *   docker-compose up -d rsfga postgres
 *   k6 run -e RSFGA_URL=http://localhost:8080 mixed-precompute.js
 *
 *   # With precompute (start Valkey + precompute worker via profile)
 *   docker-compose --profile precompute up -d
 *   k6 run -e RSFGA_URL=http://localhost:8080 mixed-precompute.js
 *
 *   Compare write throughput between the two runs — regression must be <5%.
 */

import { sleep } from 'k6';
import { createClient } from '../lib/client.js';
import { TestSetup, uniqueStoreName, randomUser, randomObject } from '../lib/setup.js';
import { recordCheck, recordWrite, errorRate } from '../lib/metrics.js';
import { Rate, Trend } from 'k6/metrics';

const BASE_URL = __ENV.RSFGA_URL || 'http://localhost:8080';
const USER_COUNT = parseInt(__ENV.USER_COUNT) || 500;
const OBJECT_COUNT = parseInt(__ENV.OBJECT_COUNT) || 50;

// Hit-rate proxy threshold (ms). This is a latency-based *approximation*,
// not an actual cache hit counter. Responses faster than this are likely
// served from Valkey rather than the graph resolver, but this can
// misclassify slow cache hits and fast misses under load.
const HIT_THRESHOLD_MS = 5;

// Timing: warmup fills hot-path, then trigger fires, then writes+checks run
const WARMUP_MAX_DURATION_S = 120;
const PRECOMPUTE_WORKER_DELAY_S = 30;
const TRIGGER_START_TIME_S = WARMUP_MAX_DURATION_S + 5;
const WORKLOAD_START_TIME_S = TRIGGER_START_TIME_S + PRECOMPUTE_WORKER_DELAY_S;

// Precompute-specific metrics
const precomputeHitRate = new Rate('precompute_hit_rate');
const precomputeLatency = new Trend('precompute_latency', true);
const mixedWriteLatency = new Trend('mixed_write_latency', true);

export const options = {
  scenarios: {
    // Stage 1: Warm-up — populate hot-path registry via checks
    warmup: {
      executor: 'shared-iterations',
      vus: 10,
      iterations: 1000,
      maxDuration: `${WARMUP_MAX_DURATION_S}s`,
      exec: 'warmup',
    },
    // Stage 2: Trigger — dummy write to fire precompute worker
    trigger: {
      executor: 'shared-iterations',
      vus: 1,
      iterations: 1,
      maxDuration: '60s',
      startTime: `${TRIGGER_START_TIME_S}s`,
      exec: 'triggerPrecompute',
    },
    // Stage 3a: Writes — sustained write workload
    writes: {
      executor: 'constant-arrival-rate',
      rate: 50,
      timeUnit: '1s',
      duration: '5m',
      preAllocatedVUs: 20,
      maxVUs: 80,
      startTime: `${WORKLOAD_START_TIME_S}s`,
      exec: 'doWrite',
    },
    // Stage 3b: Checks — sustained check workload against warm cache
    checks: {
      executor: 'constant-arrival-rate',
      rate: 200,
      timeUnit: '1s',
      duration: '5m',
      preAllocatedVUs: 50,
      maxVUs: 200,
      startTime: `${WORKLOAD_START_TIME_S}s`,
      exec: 'doCheck',
    },
  },
  thresholds: {
    // Write thresholds
    'http_req_duration{endpoint:write}': ['p(95)<50'],
    'http_req_failed{endpoint:write}': ['rate<0.01'],
    'mixed_write_latency': ['p(95)<50'],

    // Check thresholds
    'http_req_duration{endpoint:check}': ['p(95)<20', 'p(99)<50'],
    'http_req_failed{endpoint:check}': ['rate<0.001'],
    'precompute_latency': ['p(95)<10'],

    // Overall
    'error_rate': ['rate<0.01'],
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
let writeClient = null;
let checkClient = null;

export function setup() {
  const setupClient = new TestSetup(BASE_URL);
  const storeName = uniqueStoreName('mixed-precompute');
  const storeId = setupClient.createStore(storeName);
  if (!storeId) throw new Error('setup failed: createStore returned falsy');
  console.log(`Created store: ${storeId}`);

  const modelId = setupClient.writeModel(simpleModel);
  if (!modelId) throw new Error('setup failed: writeModel returned falsy');
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
  };
}

/**
 * Warm-up phase — send diverse checks to populate the hot-path registry.
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
    recordCheck(res, res.body && res.body.allowed === true);
  } else {
    errorRate.add(true);
  }
}

/**
 * Trigger — write a dummy tuple to generate a NATS committed event.
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

  sleep(PRECOMPUTE_WORKER_DELAY_S);
}

/**
 * Write workload — sustained 50 req/s during measurement phase.
 */
export function doWrite(data) {
  if (!writeClient) {
    writeClient = createClient(BASE_URL);
  }

  const { storeId, modelId, userCount } = data;
  const baseCounter = __VU * 1000000 + __ITER;

  const writes = [{
    user: randomUser(userCount),
    relation: ['viewer', 'editor'][Math.floor(Math.random() * 2)],
    object: `document:new_doc_${baseCounter}`,
  }];

  const res = writeClient.write(storeId, writes, [], modelId);
  recordWrite(res, writes.length, 0);
  mixedWriteLatency.add(res.duration);

  sleep(Math.random() * 0.01);
}

/**
 * Check workload — sustained 200 req/s during measurement phase.
 */
export function doCheck(data) {
  if (!checkClient) {
    checkClient = createClient(BASE_URL);
  }

  const { storeId, modelId, userCount, objectCount } = data;
  const user = randomUser(userCount);
  const object = randomObject('document', objectCount);
  const relation = Math.random() > 0.5 ? 'viewer' : 'editor';

  const res = checkClient.check(storeId, user, relation, object, null, modelId);

  if (res.success) {
    const allowed = res.body && res.body.allowed === true;
    recordCheck(res, allowed);
    precomputeLatency.add(res.duration);
    precomputeHitRate.add(res.duration < HIT_THRESHOLD_MS);
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
