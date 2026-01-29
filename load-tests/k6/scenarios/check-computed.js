/**
 * Check API - Computed Relations Load Test
 *
 * Tests union, intersection, and exclusion relation checks.
 * Evaluates parallel branch evaluation and short-circuit effectiveness.
 */

import { sleep } from 'k6';
import { createClient } from '../lib/client.js';
import { TestSetup, uniqueStoreName, randomUser, randomObject } from '../lib/setup.js';
import { recordCheck, errorRate } from '../lib/metrics.js';

const BASE_URL = __ENV.RSFGA_URL || 'http://localhost:8080';
const USER_COUNT = parseInt(__ENV.USER_COUNT) || 500;
const OBJECT_COUNT = parseInt(__ENV.OBJECT_COUNT) || 100;
const GROUP_COUNT = parseInt(__ENV.GROUP_COUNT) || 50;

export const options = {
  scenarios: {
    check_computed: {
      executor: 'ramping-arrival-rate',
      startRate: 10,
      timeUnit: '1s',
      preAllocatedVUs: 50,
      maxVUs: 200,
      stages: [
        { duration: '2m', target: 50 },
        { duration: '5m', target: 100 },
        { duration: '2m', target: 50 },
        { duration: '1m', target: 0 },
      ],
    },
  },
  thresholds: {
    'http_req_duration{endpoint:check}': ['p(95)<50', 'p(99)<100'],
    'http_req_failed{endpoint:check}': ['rate<0.01'],
    'check_latency': ['p(95)<50', 'p(99)<100'],
    'error_rate': ['rate<0.01'],
  },
};

let client = null;

// Complex union model (5 branches for viewer)
const complexUnionModel = {
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
      type: 'resource',
      relations: {
        owner: { this: {} },
        admin: {
          union: {
            child: [
              { this: {} },
              { computedUserset: { relation: 'owner' } },
            ],
          },
        },
        editor: {
          union: {
            child: [
              { this: {} },
              { computedUserset: { relation: 'admin' } },
            ],
          },
        },
        contributor: { this: {} },
        commenter: { this: {} },
        reviewer: { this: {} },
        viewer: {
          union: {
            child: [
              { this: {} },
              { computedUserset: { relation: 'editor' } },
              { computedUserset: { relation: 'contributor' } },
              { computedUserset: { relation: 'commenter' } },
              { computedUserset: { relation: 'reviewer' } },
            ],
          },
        },
      },
      metadata: {
        relations: {
          owner: { directly_related_user_types: [{ type: 'user' }] },
          admin: {
            directly_related_user_types: [
              { type: 'user' },
              { type: 'group', relation: 'member' },
            ],
          },
          editor: {
            directly_related_user_types: [
              { type: 'user' },
              { type: 'group', relation: 'member' },
            ],
          },
          contributor: { directly_related_user_types: [{ type: 'user' }] },
          commenter: { directly_related_user_types: [{ type: 'user' }] },
          reviewer: { directly_related_user_types: [{ type: 'user' }] },
          viewer: { directly_related_user_types: [{ type: 'user' }] },
        },
      },
    },
  ],
};

export function setup() {
  const setupClient = new TestSetup(BASE_URL);
  const storeName = uniqueStoreName('check-computed');
  const storeId = setupClient.createStore(storeName);
  console.log(`Created store: ${storeId}`);

  const modelId = setupClient.writeModel(complexUnionModel);
  console.log(`Created model: ${modelId}`);

  const tuples = [];

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

  // Assign various relations to resources
  const relations = ['owner', 'admin', 'editor', 'contributor', 'commenter', 'reviewer', 'viewer'];

  for (let o = 0; o < OBJECT_COUNT; o++) {
    // Each object gets multiple relation assignments
    const assignmentsPerObject = 5 + Math.floor(Math.random() * 10);
    for (let a = 0; a < assignmentsPerObject; a++) {
      const relation = relations[a % relations.length];
      const userOrGroup = Math.random() > 0.3
        ? `user:user_${Math.floor(Math.random() * USER_COUNT)}`
        : `group:group_${Math.floor(Math.random() * GROUP_COUNT)}#member`;

      tuples.push({
        user: userOrGroup,
        relation,
        object: `resource:resource_${o}`,
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

  const { storeId, modelId, userCount, objectCount } = data;

  const user = randomUser(userCount);
  const object = randomObject('resource', objectCount);

  // Focus on viewer relation which requires traversing 5 union branches
  const res = client.check(storeId, user, 'viewer', object, null, modelId);

  const allowed = res.success && res.body && res.body.allowed === true;
  recordCheck(res, allowed);

  sleep(Math.random() * 0.05);
}

export function teardown(data) {
  const teardownClient = createClient(BASE_URL);
  teardownClient.deleteStore(data.storeId);
  console.log(`Deleted store: ${data.storeId}`);
}
