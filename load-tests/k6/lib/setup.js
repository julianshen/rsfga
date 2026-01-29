/**
 * RSFGA Load Test Setup Helpers
 *
 * Provides utilities for creating stores, models, and tuples
 * during test setup phases.
 */

import { createClient } from './client.js';

/**
 * Setup context that persists across test iterations
 */
export class TestSetup {
  constructor(baseUrl) {
    this.client = createClient(baseUrl);
    this.storeId = null;
    this.modelId = null;
  }

  /**
   * Create a store with the given name
   */
  createStore(name) {
    const res = this.client.createStore(name);
    if (!res.success) {
      throw new Error(`Failed to create store: ${JSON.stringify(res.body)}`);
    }
    this.storeId = res.body.id;
    return this.storeId;
  }

  /**
   * Write an authorization model
   */
  writeModel(model) {
    if (!this.storeId) {
      throw new Error('Store must be created before writing model');
    }
    const res = this.client.writeAuthorizationModel(this.storeId, model);
    if (!res.success) {
      throw new Error(`Failed to write model: ${JSON.stringify(res.body)}`);
    }
    this.modelId = res.body.authorization_model_id;
    return this.modelId;
  }

  /**
   * Write tuples in batches
   */
  writeTuples(tuples, batchSize = 100) {
    if (!this.storeId) {
      throw new Error('Store must be created before writing tuples');
    }

    let written = 0;
    for (let i = 0; i < tuples.length; i += batchSize) {
      const batch = tuples.slice(i, i + batchSize);
      const res = this.client.write(this.storeId, batch, [], this.modelId);
      if (!res.success) {
        console.error(`Failed to write batch at offset ${i}: ${JSON.stringify(res.body)}`);
        // Continue with next batch
      } else {
        written += batch.length;
      }
    }
    return written;
  }

  /**
   * Clean up - delete the store
   */
  cleanup() {
    if (this.storeId) {
      this.client.deleteStore(this.storeId);
      this.storeId = null;
      this.modelId = null;
    }
  }
}

/**
 * Generate direct tuple assignments
 */
export function generateDirectTuples(userCount, objectCount, objectType = 'document', relations = ['viewer']) {
  const tuples = [];

  for (let u = 0; u < userCount; u++) {
    const userId = `user:user_${u}`;
    // Each user gets assigned to ~10% of objects
    const assignmentCount = Math.max(1, Math.floor(objectCount * 0.1));

    for (let i = 0; i < assignmentCount; i++) {
      const objectId = `${objectType}:${objectType}_${(u * assignmentCount + i) % objectCount}`;
      const relation = relations[Math.floor(Math.random() * relations.length)];

      tuples.push({
        user: userId,
        relation,
        object: objectId,
      });
    }
  }

  return tuples;
}

/**
 * Generate hierarchical tuples (parent-child relationships)
 */
export function generateHierarchyTuples(levels, itemsPerLevel, userCount) {
  const tuples = [];

  // Create parent-child relationships between levels
  for (let level = 1; level < levels; level++) {
    const parentType = level === 1 ? 'org' : `level_${level - 1}`;
    const childType = `level_${level}`;

    for (let i = 0; i < itemsPerLevel; i++) {
      const parentId = `${parentType}:${parentType}_${i % itemsPerLevel}`;
      const childId = `${childType}:${childType}_${i}`;

      tuples.push({
        user: parentId,
        relation: 'parent',
        object: childId,
      });
    }
  }

  // Assign users to the root level (org)
  for (let u = 0; u < userCount; u++) {
    const orgId = `org:org_${u % itemsPerLevel}`;
    tuples.push({
      user: `user:user_${u}`,
      relation: 'member',
      object: orgId,
    });
  }

  return tuples;
}

/**
 * Generate group membership tuples
 */
export function generateGroupTuples(groupCount, membersPerGroup, objectsPerGroup) {
  const tuples = [];

  for (let g = 0; g < groupCount; g++) {
    const groupId = `group:group_${g}`;

    // Add members to group
    for (let m = 0; m < membersPerGroup; m++) {
      tuples.push({
        user: `user:user_${g * membersPerGroup + m}`,
        relation: 'member',
        object: groupId,
      });
    }

    // Assign group to objects
    for (let o = 0; o < objectsPerGroup; o++) {
      tuples.push({
        user: `${groupId}#member`,
        relation: 'viewer',
        object: `document:document_${g * objectsPerGroup + o}`,
      });
    }
  }

  return tuples;
}

/**
 * Generate tuples with CEL conditions
 */
export function generateConditionTuples(userCount, objectCount, conditionName = 'department_condition') {
  const tuples = [];
  const departments = ['engineering', 'product', 'sales', 'marketing', 'hr'];

  for (let u = 0; u < userCount; u++) {
    const userId = `user:user_${u}`;
    const objectId = `document:document_${u % objectCount}`;
    const dept = departments[u % departments.length];

    tuples.push({
      user: userId,
      relation: 'viewer',
      object: objectId,
      condition: {
        name: conditionName,
        context: { dept },
      },
    });
  }

  return tuples;
}

/**
 * Create a unique store name with timestamp
 */
export function uniqueStoreName(prefix = 'load-test') {
  const timestamp = Date.now();
  const random = Math.random().toString(36).substring(7);
  return `${prefix}-${timestamp}-${random}`;
}

/**
 * Random selection from array
 */
export function randomFrom(arr) {
  return arr[Math.floor(Math.random() * arr.length)];
}

/**
 * Generate random user ID
 */
export function randomUser(maxUsers) {
  return `user:user_${Math.floor(Math.random() * maxUsers)}`;
}

/**
 * Generate random object ID
 */
export function randomObject(type, maxObjects) {
  return `${type}:${type}_${Math.floor(Math.random() * maxObjects)}`;
}

export default TestSetup;
