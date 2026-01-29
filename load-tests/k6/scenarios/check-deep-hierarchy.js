/**
 * Check API - Deep Hierarchy Load Test
 *
 * Tests graph traversal through 10-15 levels of hierarchy.
 * Validates depth limit handling (max 25) and traversal performance.
 */

import { sleep } from 'k6';
import { createClient } from '../lib/client.js';
import { TestSetup, uniqueStoreName, randomUser } from '../lib/setup.js';
import { recordCheck, graphDepth, depthLimitRate, errorRate } from '../lib/metrics.js';

const BASE_URL = __ENV.RSFGA_URL || 'http://localhost:8080';
const USER_COUNT = parseInt(__ENV.USER_COUNT) || 100;
const ITEMS_PER_LEVEL = parseInt(__ENV.ITEMS_PER_LEVEL) || 10;
const MAX_DEPTH = parseInt(__ENV.MAX_DEPTH) || 15;

export const options = {
  scenarios: {
    check_deep: {
      executor: 'ramping-arrival-rate',
      startRate: 10,
      timeUnit: '1s',
      preAllocatedVUs: 30,
      maxVUs: 100,
      stages: [
        { duration: '2m', target: 20 },
        { duration: '5m', target: 50 },
        { duration: '3m', target: 100 },
        { duration: '2m', target: 0 },
      ],
    },
  },
  thresholds: {
    'http_req_duration{endpoint:check}': ['p(95)<100', 'p(99)<200'],
    'http_req_failed{endpoint:check}': ['rate<0.01'],
    'check_latency': ['p(95)<100', 'p(99)<200'],
    'depth_limit_rate': ['rate<0.001'], // Should not hit depth limit with 15 levels
    'error_rate': ['rate<0.01'],
  },
};

let client = null;

// Build a deep hierarchy model dynamically
function buildDeepHierarchyModel(levels) {
  const typeDefs = [
    { type: 'user' },
    {
      type: 'org',
      relations: {
        member: { this: {} },
      },
      metadata: {
        relations: {
          member: { directly_related_user_types: [{ type: 'user' }] },
        },
      },
    },
  ];

  for (let level = 1; level <= levels; level++) {
    const parentType = level === 1 ? 'org' : `level_${level - 1}`;
    const parentRelation = level === 1 ? 'member' : 'viewer';

    typeDefs.push({
      type: `level_${level}`,
      relations: {
        parent: { this: {} },
        viewer: {
          union: {
            child: [
              { this: {} },
              {
                tupleToUserset: {
                  tupleset: { relation: 'parent' },
                  computedUserset: { relation: parentRelation },
                },
              },
            ],
          },
        },
      },
      metadata: {
        relations: {
          parent: { directly_related_user_types: [{ type: parentType }] },
          viewer: { directly_related_user_types: [{ type: 'user' }] },
        },
      },
    });
  }

  return {
    schema_version: '1.1',
    type_definitions: typeDefs,
  };
}

export function setup() {
  const setupClient = new TestSetup(BASE_URL);
  const storeName = uniqueStoreName('check-deep-hierarchy');
  const storeId = setupClient.createStore(storeName);
  console.log(`Created store: ${storeId}`);

  const model = buildDeepHierarchyModel(MAX_DEPTH);
  const modelId = setupClient.writeModel(model);
  console.log(`Created model with ${MAX_DEPTH} levels: ${modelId}`);

  const tuples = [];

  // Assign users to orgs
  for (let u = 0; u < USER_COUNT; u++) {
    tuples.push({
      user: `user:user_${u}`,
      relation: 'member',
      object: `org:org_${u % ITEMS_PER_LEVEL}`,
    });
  }

  // Create parent-child relationships between levels
  for (let level = 1; level <= MAX_DEPTH; level++) {
    const parentType = level === 1 ? 'org' : `level_${level - 1}`;

    for (let i = 0; i < ITEMS_PER_LEVEL; i++) {
      tuples.push({
        user: `${parentType}:${parentType}_${i}`,
        relation: 'parent',
        object: `level_${level}:level_${level}_${i}`,
      });
    }
  }

  const written = setupClient.writeTuples(tuples);
  console.log(`Wrote ${written} tuples`);

  return {
    storeId,
    modelId,
    userCount: USER_COUNT,
    itemsPerLevel: ITEMS_PER_LEVEL,
    maxDepth: MAX_DEPTH,
  };
}

export default function (data) {
  if (!client) {
    client = createClient(BASE_URL);
  }

  const { storeId, modelId, userCount, itemsPerLevel, maxDepth } = data;

  const user = randomUser(userCount);

  // Pick a random level (deeper = more traversal)
  // Weight toward deeper levels to stress test
  const levelDistribution = Math.random();
  let targetLevel;
  if (levelDistribution > 0.7) {
    targetLevel = maxDepth; // 30% at max depth
  } else if (levelDistribution > 0.4) {
    targetLevel = Math.floor(maxDepth * 0.7); // 30% at ~70% depth
  } else {
    targetLevel = Math.floor(Math.random() * maxDepth) + 1; // 40% random
  }

  const object = `level_${targetLevel}:level_${targetLevel}_${Math.floor(Math.random() * itemsPerLevel)}`;

  const res = client.check(storeId, user, 'viewer', object, null, modelId);

  const allowed = res.success && res.body && res.body.allowed === true;
  recordCheck(res, allowed);

  // Track graph depth
  graphDepth.add(targetLevel);

  // Check for depth limit errors
  if (res.body && res.body.code === 'depth_limit_exceeded') {
    depthLimitRate.add(true);
  } else {
    depthLimitRate.add(false);
  }

  sleep(Math.random() * 0.05);
}

export function teardown(data) {
  const teardownClient = createClient(BASE_URL);
  teardownClient.deleteStore(data.storeId);
  console.log(`Deleted store: ${data.storeId}`);
}
