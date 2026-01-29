/**
 * Batch Check API Load Test
 *
 * Tests batch check operations with deduplication.
 * Expected: >500 checks/s, visible dedup savings
 */

import { sleep } from 'k6';
import { createClient } from '../lib/client.js';
import { TestSetup, uniqueStoreName, randomUser, randomObject, randomFrom } from '../lib/setup.js';
import { recordBatchCheck, checksPerBatch, deduplicationRate, errorRate } from '../lib/metrics.js';

const BASE_URL = __ENV.RSFGA_URL || 'http://localhost:8080';
const USER_COUNT = parseInt(__ENV.USER_COUNT) || 500;
const OBJECT_COUNT = parseInt(__ENV.OBJECT_COUNT) || 200;
const BATCH_SIZE_MIN = parseInt(__ENV.BATCH_SIZE_MIN) || 25;
const BATCH_SIZE_MAX = parseInt(__ENV.BATCH_SIZE_MAX) || 50;
const DUPLICATE_RATE = parseFloat(__ENV.DUPLICATE_RATE) || 0.3;

export const options = {
  scenarios: {
    batch_check: {
      executor: 'constant-arrival-rate',
      rate: 50,
      timeUnit: '1s',
      duration: '5m',
      preAllocatedVUs: 30,
      maxVUs: 100,
    },
  },
  thresholds: {
    'http_req_duration{endpoint:batch_check}': ['p(95)<100', 'p(99)<200'],
    'http_req_failed{endpoint:batch_check}': ['rate<0.01'],
    'batch_check_latency': ['p(95)<10', 'p(99)<20'],
    'checks_per_batch': ['avg>30'],
    'error_rate': ['rate<0.01'],
  },
};

let client = null;

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

export function setup() {
  const setupClient = new TestSetup(BASE_URL);
  const storeName = uniqueStoreName('batch-check');
  const storeId = setupClient.createStore(storeName);
  console.log(`Created store: ${storeId}`);

  const modelId = setupClient.writeModel(simpleModel);
  console.log(`Created model: ${modelId}`);

  const tuples = [];
  const relations = ['viewer', 'editor'];

  // Create random assignments
  for (let i = 0; i < USER_COUNT * 2; i++) {
    tuples.push({
      user: `user:user_${Math.floor(Math.random() * USER_COUNT)}`,
      relation: relations[i % 2],
      object: `document:doc_${Math.floor(Math.random() * OBJECT_COUNT)}`,
    });
  }

  const written = setupClient.writeTuples(tuples);
  console.log(`Wrote ${written} tuples`);

  return {
    storeId,
    modelId,
    userCount: USER_COUNT,
    objectCount: OBJECT_COUNT,
    batchSizeMin: BATCH_SIZE_MIN,
    batchSizeMax: BATCH_SIZE_MAX,
    duplicateRate: DUPLICATE_RATE,
  };
}

export default function (data) {
  if (!client) {
    client = createClient(BASE_URL);
  }

  const { storeId, modelId, userCount, objectCount, batchSizeMin, batchSizeMax, duplicateRate } = data;

  // Random batch size
  const batchSize = batchSizeMin + Math.floor(Math.random() * (batchSizeMax - batchSizeMin));

  // Build batch with some duplicates
  const checks = [];
  const seenChecks = new Set();

  for (let i = 0; i < batchSize; i++) {
    let check;

    // Introduce duplicates based on rate
    if (checks.length > 0 && Math.random() < duplicateRate) {
      // Pick a random existing check as duplicate
      check = checks[Math.floor(Math.random() * checks.length)];
    } else {
      // Create new unique check
      check = {
        user: randomUser(userCount),
        relation: Math.random() > 0.5 ? 'viewer' : 'editor',
        object: randomObject('document', objectCount),
        correlationId: `check_${i}`,
      };
    }

    checks.push(check);
    seenChecks.add(`${check.user}:${check.relation}:${check.object}`);
  }

  const res = client.batchCheck(storeId, checks, modelId);

  // Record metrics
  const resultsCount = res.body && res.body.results ? Object.keys(res.body.results).length : checks.length;
  recordBatchCheck(res, checks.length, resultsCount);
  checksPerBatch.add(checks.length);

  // Calculate actual deduplication
  const uniqueChecks = seenChecks.size;
  const hadDuplicates = uniqueChecks < checks.length;
  deduplicationRate.add(hadDuplicates);

  sleep(Math.random() * 0.02);
}

export function teardown(data) {
  const teardownClient = createClient(BASE_URL);
  teardownClient.deleteStore(data.storeId);
  console.log(`Deleted store: ${data.storeId}`);
}
