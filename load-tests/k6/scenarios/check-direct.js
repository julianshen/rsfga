/**
 * Check API - Direct Relations Load Test
 *
 * Tests direct relation checks (no graph traversal).
 * Expected: >90% cache hit, <5ms p95 latency
 */

import { sleep } from 'k6';
import { createClient } from '../lib/client.js';
import { TestSetup, uniqueStoreName, randomUser, randomObject } from '../lib/setup.js';
import { recordCheck, errorRate } from '../lib/metrics.js';

// Configuration from environment or defaults
const BASE_URL = __ENV.RSFGA_URL || 'http://localhost:8080';
const USER_COUNT = parseInt(__ENV.USER_COUNT) || 1000;
const OBJECT_COUNT = parseInt(__ENV.OBJECT_COUNT) || 100;

// Test options
export const options = {
  scenarios: {
    check_direct: {
      executor: 'constant-arrival-rate',
      rate: 100,
      timeUnit: '1s',
      duration: '5m',
      preAllocatedVUs: 50,
      maxVUs: 200,
    },
  },
  thresholds: {
    'http_req_duration{endpoint:check}': ['p(95)<5', 'p(99)<20'],
    'http_req_failed{endpoint:check}': ['rate<0.001'],
    'check_latency': ['p(95)<5', 'p(99)<20'],
    'cache_hit_rate': ['rate>0.9'],
    'error_rate': ['rate<0.001'],
  },
};

// Shared state for VU execution
let client = null;

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
        owner: { this: {} },
      },
      metadata: {
        relations: {
          viewer: { directly_related_user_types: [{ type: 'user' }] },
          editor: { directly_related_user_types: [{ type: 'user' }] },
          owner: { directly_related_user_types: [{ type: 'user' }] },
        },
      },
    },
  ],
};

/**
 * Setup function - runs once before all VUs
 */
export function setup() {
  const setupClient = new TestSetup(BASE_URL);

  // Create store
  const storeName = uniqueStoreName('check-direct');
  const storeId = setupClient.createStore(storeName);
  console.log(`Created store: ${storeId}`);

  // Write model
  const modelId = setupClient.writeModel(simpleModel);
  console.log(`Created model: ${modelId}`);

  // Generate and write tuples
  const tuples = [];
  const relations = ['viewer', 'editor', 'owner'];

  for (let u = 0; u < USER_COUNT; u++) {
    // Each user gets direct access to ~10 objects
    const objectsPerUser = Math.floor(OBJECT_COUNT * 0.1);
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
 * Main test function - runs by each VU
 */
export default function (data) {
  if (!client) {
    client = createClient(BASE_URL);
  }

  const { storeId, modelId, userCount, objectCount } = data;

  // Random user and object
  const user = randomUser(userCount);
  const object = randomObject('document', objectCount);
  const relation = ['viewer', 'editor', 'owner'][Math.floor(Math.random() * 3)];

  // Perform check
  const res = client.check(storeId, user, relation, object, null, modelId);

  // Record metrics
  const allowed = res.success && res.body && res.body.allowed === true;
  recordCheck(res, allowed);

  // Small random sleep to avoid thundering herd
  sleep(Math.random() * 0.1);
}

/**
 * Teardown function - runs once after all VUs complete
 */
export function teardown(data) {
  const teardownClient = createClient(BASE_URL);
  teardownClient.deleteStore(data.storeId);
  console.log(`Deleted store: ${data.storeId}`);
}
