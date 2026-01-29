/**
 * ListObjects API Load Test
 *
 * Tests ListObjects with users having access to 100-1000 objects.
 * Expected: >100 req/s, pagination working correctly.
 */

import { sleep } from 'k6';
import { createClient } from '../lib/client.js';
import { TestSetup, uniqueStoreName, randomUser } from '../lib/setup.js';
import { recordListObjects, objectsReturned, errorRate } from '../lib/metrics.js';

const BASE_URL = __ENV.RSFGA_URL || 'http://localhost:8080';
const USER_COUNT = parseInt(__ENV.USER_COUNT) || 100;
const OBJECT_COUNT = parseInt(__ENV.OBJECT_COUNT) || 1000;
const OBJECTS_PER_USER_MIN = parseInt(__ENV.OBJECTS_PER_USER_MIN) || 100;
const OBJECTS_PER_USER_MAX = parseInt(__ENV.OBJECTS_PER_USER_MAX) || 500;

export const options = {
  scenarios: {
    list_objects: {
      executor: 'constant-arrival-rate',
      rate: 20,
      timeUnit: '1s',
      duration: '5m',
      preAllocatedVUs: 20,
      maxVUs: 50,
    },
  },
  thresholds: {
    'http_req_duration{endpoint:list_objects}': ['p(95)<200', 'p(99)<500'],
    'http_req_failed{endpoint:list_objects}': ['rate<0.01'],
    'list_objects_latency': ['p(95)<200', 'p(99)<500'],
    'objects_returned': ['avg>50'],
    'error_rate': ['rate<0.01'],
  },
};

let client = null;

const simpleModel = {
  schema_version: '1.1',
  type_definitions: [
    { type: 'user' },
    {
      type: 'folder',
      relations: {
        viewer: { this: {} },
      },
      metadata: {
        relations: {
          viewer: { directly_related_user_types: [{ type: 'user' }] },
        },
      },
    },
    {
      type: 'document',
      relations: {
        parent: { this: {} },
        viewer: {
          union: {
            child: [
              { this: {} },
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
          viewer: { directly_related_user_types: [{ type: 'user' }] },
        },
      },
    },
  ],
};

export function setup() {
  const setupClient = new TestSetup(BASE_URL);
  const storeName = uniqueStoreName('list-objects');
  const storeId = setupClient.createStore(storeName);
  console.log(`Created store: ${storeId}`);

  const modelId = setupClient.writeModel(simpleModel);
  console.log(`Created model: ${modelId}`);

  const tuples = [];
  const folderCount = 50;

  // Create folders
  for (let f = 0; f < folderCount; f++) {
    // Assign some users as folder viewers
    const usersPerFolder = Math.floor(USER_COUNT / folderCount) + 5;
    for (let u = 0; u < usersPerFolder; u++) {
      tuples.push({
        user: `user:user_${(f * usersPerFolder + u) % USER_COUNT}`,
        relation: 'viewer',
        object: `folder:folder_${f}`,
      });
    }
  }

  // Create documents in folders
  for (let d = 0; d < OBJECT_COUNT; d++) {
    tuples.push({
      user: `folder:folder_${d % folderCount}`,
      relation: 'parent',
      object: `document:doc_${d}`,
    });

    // Some direct assignments too
    if (d % 10 === 0) {
      tuples.push({
        user: `user:user_${d % USER_COUNT}`,
        relation: 'viewer',
        object: `document:doc_${d}`,
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

export default function (data) {
  if (!client) {
    client = createClient(BASE_URL);
  }

  const { storeId, modelId, userCount } = data;

  const user = randomUser(userCount);

  const res = client.listObjects(storeId, user, 'viewer', 'document', null, modelId);

  const objectCount = res.body && res.body.objects ? res.body.objects.length : 0;
  recordListObjects(res, objectCount);
  objectsReturned.add(objectCount);

  sleep(Math.random() * 0.05);
}

export function teardown(data) {
  const teardownClient = createClient(BASE_URL);
  teardownClient.deleteStore(data.storeId);
  console.log(`Deleted store: ${data.storeId}`);
}
