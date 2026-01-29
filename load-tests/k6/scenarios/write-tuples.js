/**
 * Write API Load Test
 *
 * Tests write throughput for tuples (writes and deletes).
 * Expected: >150 req/s sustained
 */

import { sleep } from 'k6';
import { createClient } from '../lib/client.js';
import { TestSetup, uniqueStoreName } from '../lib/setup.js';
import { recordWrite, writeThroughput, tuplesWritten, tuplesDeleted, errorRate } from '../lib/metrics.js';

const BASE_URL = __ENV.RSFGA_URL || 'http://localhost:8080';
const USER_COUNT = parseInt(__ENV.USER_COUNT) || 1000;
const OBJECT_COUNT = parseInt(__ENV.OBJECT_COUNT) || 500;
const WRITE_BATCH_SIZE = parseInt(__ENV.WRITE_BATCH_SIZE) || 10;
const DELETE_RATIO = parseFloat(__ENV.DELETE_RATIO) || 0.2;

export const options = {
  scenarios: {
    write_tuples: {
      executor: 'ramping-arrival-rate',
      startRate: 10,
      timeUnit: '1s',
      preAllocatedVUs: 30,
      maxVUs: 100,
      stages: [
        { duration: '1m', target: 50 },
        { duration: '3m', target: 100 },
        { duration: '3m', target: 150 },
        { duration: '2m', target: 200 },
        { duration: '1m', target: 0 },
      ],
    },
  },
  thresholds: {
    'http_req_duration{endpoint:write}': ['p(95)<50', 'p(99)<100'],
    'http_req_failed{endpoint:write}': ['rate<0.01'],
    'write_latency': ['p(95)<50', 'p(99)<100'],
    'write_throughput': ['avg>100'],
    'error_rate': ['rate<0.01'],
  },
};

let client = null;
let tupleCounter = 0;

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

export function setup() {
  const setupClient = new TestSetup(BASE_URL);
  const storeName = uniqueStoreName('write-tuples');
  const storeId = setupClient.createStore(storeName);
  console.log(`Created store: ${storeId}`);

  const modelId = setupClient.writeModel(simpleModel);
  console.log(`Created model: ${modelId}`);

  // Pre-populate some tuples for delete operations
  const initialTuples = [];
  for (let i = 0; i < 1000; i++) {
    initialTuples.push({
      user: `user:user_${i % USER_COUNT}`,
      relation: 'viewer',
      object: `document:initial_doc_${i}`,
    });
  }

  setupClient.writeTuples(initialTuples);
  console.log('Wrote initial tuples for delete operations');

  return {
    storeId,
    modelId,
    userCount: USER_COUNT,
    objectCount: OBJECT_COUNT,
    batchSize: WRITE_BATCH_SIZE,
    deleteRatio: DELETE_RATIO,
  };
}

export default function (data) {
  if (!client) {
    client = createClient(BASE_URL);
  }

  const { storeId, modelId, userCount, objectCount, batchSize, deleteRatio } = data;

  // Increment counter to generate unique tuples
  const baseCounter = __VU * 1000000 + tupleCounter;
  tupleCounter += batchSize;

  const isDelete = Math.random() < deleteRatio;
  const relations = ['viewer', 'editor', 'owner'];

  if (isDelete) {
    // Delete existing tuples
    const deletes = [];
    for (let i = 0; i < batchSize; i++) {
      deletes.push({
        user: `user:user_${(baseCounter + i) % userCount}`,
        relation: 'viewer',
        object: `document:initial_doc_${(baseCounter + i) % 1000}`,
      });
    }

    const res = client.write(storeId, [], deletes, modelId);
    recordWrite(res, 0, deletes.length);
    tuplesDeleted.add(deletes.length);
  } else {
    // Write new tuples
    const writes = [];
    for (let i = 0; i < batchSize; i++) {
      writes.push({
        user: `user:user_${(baseCounter + i) % userCount}`,
        relation: relations[i % 3],
        object: `document:doc_${baseCounter + i}`,
      });
    }

    const res = client.write(storeId, writes, [], modelId);
    recordWrite(res, writes.length, 0);
    tuplesWritten.add(writes.length);
  }

  sleep(Math.random() * 0.01);
}

export function teardown(data) {
  const teardownClient = createClient(BASE_URL);
  teardownClient.deleteStore(data.storeId);
  console.log(`Deleted store: ${data.storeId}`);
}
