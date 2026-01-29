/**
 * ListUsers API Load Test
 *
 * Tests ListUsers with wildcard expansion and userset expansion.
 * Measures truncation behavior for large result sets.
 */

import { sleep } from 'k6';
import { createClient } from '../lib/client.js';
import { TestSetup, uniqueStoreName, randomObject } from '../lib/setup.js';
import { recordListUsers, usersReturned, errorRate } from '../lib/metrics.js';

const BASE_URL = __ENV.RSFGA_URL || 'http://localhost:8080';
const USER_COUNT = parseInt(__ENV.USER_COUNT) || 200;
const GROUP_COUNT = parseInt(__ENV.GROUP_COUNT) || 20;
const OBJECT_COUNT = parseInt(__ENV.OBJECT_COUNT) || 100;
const MEMBERS_PER_GROUP = parseInt(__ENV.MEMBERS_PER_GROUP) || 50;

export const options = {
  scenarios: {
    list_users: {
      executor: 'constant-arrival-rate',
      rate: 20,
      timeUnit: '1s',
      duration: '5m',
      preAllocatedVUs: 20,
      maxVUs: 50,
    },
  },
  thresholds: {
    'http_req_duration{endpoint:list_users}': ['p(95)<300', 'p(99)<500'],
    'http_req_failed{endpoint:list_users}': ['rate<0.01'],
    'list_users_latency': ['p(95)<300', 'p(99)<500'],
    'users_returned': ['avg>10'],
    'error_rate': ['rate<0.01'],
  },
};

let client = null;

const groupModel = {
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
      type: 'document',
      relations: {
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
            ],
          },
        },
      },
      metadata: {
        relations: {
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
              { type: 'user', wildcard: {} },
            ],
          },
        },
      },
    },
  ],
};

export function setup() {
  const setupClient = new TestSetup(BASE_URL);
  const storeName = uniqueStoreName('list-users');
  const storeId = setupClient.createStore(storeName);
  console.log(`Created store: ${storeId}`);

  const modelId = setupClient.writeModel(groupModel);
  console.log(`Created model: ${modelId}`);

  const tuples = [];

  // Create groups with members
  for (let g = 0; g < GROUP_COUNT; g++) {
    for (let m = 0; m < MEMBERS_PER_GROUP; m++) {
      tuples.push({
        user: `user:user_${(g * MEMBERS_PER_GROUP + m) % USER_COUNT}`,
        relation: 'member',
        object: `group:group_${g}`,
      });
    }
  }

  // Assign permissions to documents
  for (let d = 0; d < OBJECT_COUNT; d++) {
    // Direct owner
    tuples.push({
      user: `user:user_${d % USER_COUNT}`,
      relation: 'owner',
      object: `document:doc_${d}`,
    });

    // Group as editor
    tuples.push({
      user: `group:group_${d % GROUP_COUNT}#member`,
      relation: 'editor',
      object: `document:doc_${d}`,
    });

    // Some documents have wildcard viewer
    if (d % 5 === 0) {
      tuples.push({
        user: 'user:*',
        relation: 'viewer',
        object: `document:doc_${d}`,
      });
    }

    // Direct viewers
    const directViewers = 3 + Math.floor(Math.random() * 5);
    for (let v = 0; v < directViewers; v++) {
      tuples.push({
        user: `user:user_${(d + v) % USER_COUNT}`,
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
    objectCount: OBJECT_COUNT,
  };
}

export default function (data) {
  if (!client) {
    client = createClient(BASE_URL);
  }

  const { storeId, modelId, objectCount } = data;

  const objectId = Math.floor(Math.random() * objectCount);
  const object = { type: 'document', id: `doc_${objectId}` };

  // List users with user type filter
  const userFilters = [{ type: 'user' }];

  const res = client.listUsers(storeId, object, 'viewer', userFilters, null, modelId);

  const userCount = res.body && res.body.users ? res.body.users.length : 0;
  recordListUsers(res, userCount);
  usersReturned.add(userCount);

  sleep(Math.random() * 0.05);
}

export function teardown(data) {
  const teardownClient = createClient(BASE_URL);
  teardownClient.deleteStore(data.storeId);
  console.log(`Deleted store: ${data.storeId}`);
}
