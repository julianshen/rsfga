/**
 * Expand API Load Test
 *
 * Tests the Expand API that shows the relationship tree.
 * Useful for debugging and understanding permission inheritance.
 */

import { sleep } from 'k6';
import { createClient } from '../lib/client.js';
import { TestSetup, uniqueStoreName, randomObject } from '../lib/setup.js';
import { recordExpand, errorRate } from '../lib/metrics.js';

const BASE_URL = __ENV.RSFGA_URL || 'http://localhost:8080';
const USER_COUNT = parseInt(__ENV.USER_COUNT) || 100;
const GROUP_COUNT = parseInt(__ENV.GROUP_COUNT) || 20;
const OBJECT_COUNT = parseInt(__ENV.OBJECT_COUNT) || 100;

export const options = {
  scenarios: {
    expand: {
      executor: 'constant-arrival-rate',
      rate: 30,
      timeUnit: '1s',
      duration: '5m',
      preAllocatedVUs: 20,
      maxVUs: 50,
    },
  },
  thresholds: {
    'http_req_duration{endpoint:expand}': ['p(95)<100', 'p(99)<200'],
    'http_req_failed{endpoint:expand}': ['rate<0.01'],
    'expand_latency': ['p(95)<100', 'p(99)<200'],
    'error_rate': ['rate<0.01'],
  },
};

let client = null;

const complexModel = {
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
  const storeName = uniqueStoreName('expand');
  const storeId = setupClient.createStore(storeName);
  console.log(`Created store: ${storeId}`);

  const modelId = setupClient.writeModel(complexModel);
  console.log(`Created model: ${modelId}`);

  const tuples = [];
  const folderCount = 20;

  // Create groups with members
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

  // Create folders with viewers
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

  // Create documents in folders
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

    // Add some direct viewers
    for (let v = 0; v < 3; v++) {
      tuples.push({
        user: `user:user_${(d + v + 1) % USER_COUNT}`,
        relation: 'viewer',
        object: `document:doc_${d}`,
      });
    }

    // Add group as editor
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
    objectCount: OBJECT_COUNT,
  };
}

export default function (data) {
  if (!client) {
    client = createClient(BASE_URL);
  }

  const { storeId, modelId, objectCount } = data;

  const object = randomObject('document', objectCount);
  const relations = ['viewer', 'editor', 'owner'];
  const relation = relations[Math.floor(Math.random() * relations.length)];

  const res = client.expand(storeId, relation, object, modelId);

  recordExpand(res);

  sleep(Math.random() * 0.03);
}

export function teardown(data) {
  const teardownClient = createClient(BASE_URL);
  teardownClient.deleteStore(data.storeId);
  console.log(`Deleted store: ${data.storeId}`);
}
