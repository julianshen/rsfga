/**
 * Mixed Workload Load Test
 *
 * Simulates realistic production traffic:
 * - 60% checks
 * - 25% batch checks
 * - 10% list operations (objects + users)
 * - 5% writes
 *
 * Total: 500 req/s for 10 minutes
 */

import { sleep } from 'k6';
import { createClient } from '../lib/client.js';
import { TestSetup, uniqueStoreName, randomUser, randomObject } from '../lib/setup.js';
import {
  recordCheck,
  recordBatchCheck,
  recordListObjects,
  recordListUsers,
  recordWrite,
  errorRate,
} from '../lib/metrics.js';

const BASE_URL = __ENV.RSFGA_URL || 'http://localhost:8080';
const USER_COUNT = parseInt(__ENV.USER_COUNT) || 500;
const OBJECT_COUNT = parseInt(__ENV.OBJECT_COUNT) || 200;
const GROUP_COUNT = parseInt(__ENV.GROUP_COUNT) || 30;

export const options = {
  scenarios: {
    checks: {
      executor: 'constant-arrival-rate',
      rate: 300, // 60% of 500
      timeUnit: '1s',
      duration: '10m',
      preAllocatedVUs: 100,
      maxVUs: 300,
      exec: 'doCheck',
    },
    batch_checks: {
      executor: 'constant-arrival-rate',
      rate: 125, // 25% of 500
      timeUnit: '1s',
      duration: '10m',
      preAllocatedVUs: 50,
      maxVUs: 150,
      exec: 'doBatchCheck',
    },
    list_objects: {
      executor: 'constant-arrival-rate',
      rate: 25, // 5% of 500
      timeUnit: '1s',
      duration: '10m',
      preAllocatedVUs: 20,
      maxVUs: 50,
      exec: 'doListObjects',
    },
    list_users: {
      executor: 'constant-arrival-rate',
      rate: 25, // 5% of 500
      timeUnit: '1s',
      duration: '10m',
      preAllocatedVUs: 20,
      maxVUs: 50,
      exec: 'doListUsers',
    },
    writes: {
      executor: 'constant-arrival-rate',
      rate: 25, // 5% of 500
      timeUnit: '1s',
      duration: '10m',
      preAllocatedVUs: 15,
      maxVUs: 50,
      exec: 'doWrite',
    },
  },
  thresholds: {
    // Check thresholds
    'http_req_duration{endpoint:check}': ['p(95)<20', 'p(99)<50'],
    'http_req_failed{endpoint:check}': ['rate<0.001'],

    // Batch check thresholds
    'http_req_duration{endpoint:batch_check}': ['p(95)<100'],
    'http_req_failed{endpoint:batch_check}': ['rate<0.01'],

    // ListObjects thresholds
    'http_req_duration{endpoint:list_objects}': ['p(95)<200'],
    'http_req_failed{endpoint:list_objects}': ['rate<0.01'],

    // ListUsers thresholds
    'http_req_duration{endpoint:list_users}': ['p(95)<300'],
    'http_req_failed{endpoint:list_users}': ['rate<0.01'],

    // Write thresholds
    'http_req_duration{endpoint:write}': ['p(95)<50'],
    'http_req_failed{endpoint:write}': ['rate<0.01'],

    // Overall
    'error_rate': ['rate<0.01'],
  },
};

let client = null;
let writeCounter = 0;

// Comprehensive model for mixed workload
const mixedModel = {
  schema_version: '1.1',
  type_definitions: [
    { type: 'user' },
    {
      type: 'group',
      relations: {
        member: { this: {} },
      },
      metadata: {
        relations: {
          member: { directly_related_user_types: [{ type: 'user' }] },
        },
      },
    },
    {
      type: 'folder',
      relations: {
        owner: { this: {} },
        viewer: {
          union: {
            child: [
              { this: {} },
              { computedUserset: { relation: 'owner' } },
            ],
          },
        },
      },
      metadata: {
        relations: {
          owner: { directly_related_user_types: [{ type: 'user' }] },
          viewer: {
            directly_related_user_types: [
              { type: 'user' },
              { type: 'group', relation: 'member' },
            ],
          },
        },
      },
    },
    {
      type: 'document',
      relations: {
        parent: { this: {} },
        owner: { this: {} },
        editor: {
          union: {
            child: [
              { this: {} },
              { computedUserset: { relation: 'owner' } },
            ],
          },
        },
        viewer: {
          union: {
            child: [
              { this: {} },
              { computedUserset: { relation: 'editor' } },
              {
                tupleToUserset: {
                  tupleset: { relation: 'parent' },
                  computedUserset: { relation: 'viewer' },
                },
              },
            ],
          },
        },
      },
      metadata: {
        relations: {
          parent: { directly_related_user_types: [{ type: 'folder' }] },
          owner: { directly_related_user_types: [{ type: 'user' }] },
          editor: {
            directly_related_user_types: [
              { type: 'user' },
              { type: 'group', relation: 'member' },
            ],
          },
          viewer: {
            directly_related_user_types: [
              { type: 'user' },
              { type: 'group', relation: 'member' },
            ],
          },
        },
      },
    },
  ],
};

export function setup() {
  const setupClient = new TestSetup(BASE_URL);
  const storeName = uniqueStoreName('mixed-workload');
  const storeId = setupClient.createStore(storeName);
  console.log(`Created store: ${storeId}`);

  const modelId = setupClient.writeModel(mixedModel);
  console.log(`Created model: ${modelId}`);

  const tuples = [];
  const folderCount = 50;

  // Create groups
  for (let g = 0; g < GROUP_COUNT; g++) {
    const membersPerGroup = Math.floor(USER_COUNT / GROUP_COUNT);
    for (let m = 0; m < membersPerGroup; m++) {
      tuples.push({
        user: `user:user_${g * membersPerGroup + m}`,
        relation: 'member',
        object: `group:group_${g}`,
      });
    }
  }

  // Create folders
  for (let f = 0; f < folderCount; f++) {
    tuples.push({
      user: `user:user_${f % USER_COUNT}`,
      relation: 'owner',
      object: `folder:folder_${f}`,
    });

    tuples.push({
      user: `group:group_${f % GROUP_COUNT}#member`,
      relation: 'viewer',
      object: `folder:folder_${f}`,
    });
  }

  // Create documents
  for (let d = 0; d < OBJECT_COUNT; d++) {
    tuples.push({
      user: `folder:folder_${d % folderCount}`,
      relation: 'parent',
      object: `document:doc_${d}`,
    });

    tuples.push({
      user: `user:user_${d % USER_COUNT}`,
      relation: 'owner',
      object: `document:doc_${d}`,
    });

    // Direct viewers
    for (let v = 0; v < 3; v++) {
      tuples.push({
        user: `user:user_${(d + v + 1) % USER_COUNT}`,
        relation: 'viewer',
        object: `document:doc_${d}`,
      });
    }

    // Group editors
    tuples.push({
      user: `group:group_${d % GROUP_COUNT}#member`,
      relation: 'editor',
      object: `document:doc_${d}`,
    });
  }

  const written = setupClient.writeTuples(tuples);
  console.log(`Wrote ${written} tuples`);

  return {
    storeId,
    modelId,
    userCount: USER_COUNT,
    objectCount: OBJECT_COUNT,
    groupCount: GROUP_COUNT,
  };
}

function getClient() {
  if (!client) {
    client = createClient(BASE_URL);
  }
  return client;
}

export function doCheck(data) {
  const c = getClient();
  const { storeId, modelId, userCount, objectCount } = data;

  const user = randomUser(userCount);
  const object = randomObject('document', objectCount);
  const relation = ['viewer', 'editor', 'owner'][Math.floor(Math.random() * 3)];

  const res = c.check(storeId, user, relation, object, null, modelId);
  const allowed = res.success && res.body && res.body.allowed === true;
  recordCheck(res, allowed);

  sleep(Math.random() * 0.01);
}

export function doBatchCheck(data) {
  const c = getClient();
  const { storeId, modelId, userCount, objectCount } = data;

  const batchSize = 20 + Math.floor(Math.random() * 30);
  const checks = [];

  for (let i = 0; i < batchSize; i++) {
    checks.push({
      user: randomUser(userCount),
      relation: Math.random() > 0.5 ? 'viewer' : 'editor',
      object: randomObject('document', objectCount),
      correlationId: `check_${i}`,
    });
  }

  const res = c.batchCheck(storeId, checks, modelId);
  const resultsCount = res.body && res.body.results ? Object.keys(res.body.results).length : checks.length;
  recordBatchCheck(res, checks.length, resultsCount);

  sleep(Math.random() * 0.01);
}

export function doListObjects(data) {
  const c = getClient();
  const { storeId, modelId, userCount } = data;

  const user = randomUser(userCount);
  const res = c.listObjects(storeId, user, 'viewer', 'document', null, modelId);
  const objectCount = res.body && res.body.objects ? res.body.objects.length : 0;
  recordListObjects(res, objectCount);

  sleep(Math.random() * 0.02);
}

export function doListUsers(data) {
  const c = getClient();
  const { storeId, modelId, objectCount } = data;

  const object = { type: 'document', id: `doc_${Math.floor(Math.random() * objectCount)}` };
  const res = c.listUsers(storeId, object, 'viewer', [{ type: 'user' }], null, modelId);
  const userCount = res.body && res.body.users ? res.body.users.length : 0;
  recordListUsers(res, userCount);

  sleep(Math.random() * 0.02);
}

export function doWrite(data) {
  const c = getClient();
  const { storeId, modelId, userCount, objectCount } = data;

  const baseCounter = __VU * 1000000 + writeCounter;
  writeCounter++;

  const writes = [{
    user: randomUser(userCount),
    relation: ['viewer', 'editor'][Math.floor(Math.random() * 2)],
    object: `document:new_doc_${baseCounter}`,
  }];

  const res = c.write(storeId, writes, [], modelId);
  recordWrite(res, writes.length, 0);

  sleep(Math.random() * 0.01);
}

export function teardown(data) {
  const teardownClient = createClient(BASE_URL);
  teardownClient.deleteStore(data.storeId);
  console.log(`Deleted store: ${data.storeId}`);
}
