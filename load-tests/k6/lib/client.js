/**
 * RSFGA HTTP Client Library for k6
 *
 * Provides a clean interface to all RSFGA REST API endpoints.
 */

import http from 'k6/http';
import { check } from 'k6';

const JSON_HEADERS = {
  'Content-Type': 'application/json',
};

/**
 * RSFGA API Client
 */
export class RsfgaClient {
  constructor(baseUrl) {
    this.baseUrl = baseUrl.replace(/\/$/, '');
  }

  // ============ Store Operations ============

  /**
   * Create a new store
   */
  createStore(name) {
    const url = `${this.baseUrl}/stores`;
    const payload = JSON.stringify({ name });
    const res = http.post(url, payload, { headers: JSON_HEADERS, tags: { endpoint: 'create_store' } });
    return this._parseResponse(res);
  }

  /**
   * List all stores
   */
  listStores(pageSize = 100, continuationToken = null) {
    let url = `${this.baseUrl}/stores?page_size=${pageSize}`;
    if (continuationToken) {
      url += `&continuation_token=${encodeURIComponent(continuationToken)}`;
    }
    const res = http.get(url, { tags: { endpoint: 'list_stores' } });
    return this._parseResponse(res);
  }

  /**
   * Get a specific store
   */
  getStore(storeId) {
    const url = `${this.baseUrl}/stores/${storeId}`;
    const res = http.get(url, { tags: { endpoint: 'get_store' } });
    return this._parseResponse(res);
  }

  /**
   * Delete a store
   */
  deleteStore(storeId) {
    const url = `${this.baseUrl}/stores/${storeId}`;
    const res = http.del(url, null, { tags: { endpoint: 'delete_store' } });
    return { status: res.status, success: res.status === 204 };
  }

  // ============ Authorization Model Operations ============

  /**
   * Write an authorization model
   */
  writeAuthorizationModel(storeId, model) {
    const url = `${this.baseUrl}/stores/${storeId}/authorization-models`;
    const payload = JSON.stringify(model);
    const res = http.post(url, payload, { headers: JSON_HEADERS, tags: { endpoint: 'write_model' } });
    return this._parseResponse(res);
  }

  /**
   * Read an authorization model
   */
  readAuthorizationModel(storeId, modelId) {
    const url = `${this.baseUrl}/stores/${storeId}/authorization-models/${modelId}`;
    const res = http.get(url, { tags: { endpoint: 'read_model' } });
    return this._parseResponse(res);
  }

  /**
   * List authorization models
   */
  listAuthorizationModels(storeId, pageSize = 100, continuationToken = null) {
    let url = `${this.baseUrl}/stores/${storeId}/authorization-models?page_size=${pageSize}`;
    if (continuationToken) {
      url += `&continuation_token=${encodeURIComponent(continuationToken)}`;
    }
    const res = http.get(url, { tags: { endpoint: 'list_models' } });
    return this._parseResponse(res);
  }

  // ============ Tuple Operations ============

  /**
   * Write relationship tuples
   */
  write(storeId, writes, deletes = [], authModelId = null) {
    const url = `${this.baseUrl}/stores/${storeId}/write`;
    const payload = {
      writes: writes.length > 0 ? { tuple_keys: writes } : undefined,
      deletes: deletes.length > 0 ? { tuple_keys: deletes } : undefined,
    };
    if (authModelId) {
      payload.authorization_model_id = authModelId;
    }
    const res = http.post(url, JSON.stringify(payload), {
      headers: JSON_HEADERS,
      tags: { endpoint: 'write' }
    });
    return this._parseResponse(res);
  }

  /**
   * Read relationship tuples
   */
  read(storeId, tupleKey = {}, pageSize = 100, continuationToken = null) {
    const url = `${this.baseUrl}/stores/${storeId}/read`;
    const payload = {
      tuple_key: tupleKey,
      page_size: pageSize,
    };
    if (continuationToken) {
      payload.continuation_token = continuationToken;
    }
    const res = http.post(url, JSON.stringify(payload), {
      headers: JSON_HEADERS,
      tags: { endpoint: 'read' }
    });
    return this._parseResponse(res);
  }

  // ============ Check Operations ============

  /**
   * Check if a user has a relation to an object
   */
  check(storeId, user, relation, object, context = null, authModelId = null) {
    const url = `${this.baseUrl}/stores/${storeId}/check`;
    const payload = {
      tuple_key: { user, relation, object },
    };
    if (context) {
      payload.context = context;
    }
    if (authModelId) {
      payload.authorization_model_id = authModelId;
    }
    const res = http.post(url, JSON.stringify(payload), {
      headers: JSON_HEADERS,
      tags: { endpoint: 'check' }
    });
    return this._parseResponse(res);
  }

  /**
   * Batch check multiple tuples
   */
  batchCheck(storeId, checks, authModelId = null) {
    const url = `${this.baseUrl}/stores/${storeId}/batch-check`;
    const payload = {
      checks: checks.map(c => ({
        tuple_key: { user: c.user, relation: c.relation, object: c.object },
        context: c.context,
        correlation_id: c.correlationId,
      })),
    };
    if (authModelId) {
      payload.authorization_model_id = authModelId;
    }
    const res = http.post(url, JSON.stringify(payload), {
      headers: JSON_HEADERS,
      tags: { endpoint: 'batch_check' }
    });
    return this._parseResponse(res);
  }

  // ============ Expand Operation ============

  /**
   * Expand a relation (show the relationship tree)
   */
  expand(storeId, relation, object, authModelId = null) {
    const url = `${this.baseUrl}/stores/${storeId}/expand`;
    const payload = {
      tuple_key: { relation, object },
    };
    if (authModelId) {
      payload.authorization_model_id = authModelId;
    }
    const res = http.post(url, JSON.stringify(payload), {
      headers: JSON_HEADERS,
      tags: { endpoint: 'expand' }
    });
    return this._parseResponse(res);
  }

  // ============ ListObjects Operation ============

  /**
   * List objects a user has access to
   */
  listObjects(storeId, user, relation, type, context = null, authModelId = null) {
    const url = `${this.baseUrl}/stores/${storeId}/list-objects`;
    const payload = { user, relation, type };
    if (context) {
      payload.context = context;
    }
    if (authModelId) {
      payload.authorization_model_id = authModelId;
    }
    const res = http.post(url, JSON.stringify(payload), {
      headers: JSON_HEADERS,
      tags: { endpoint: 'list_objects' }
    });
    return this._parseResponse(res);
  }

  // ============ ListUsers Operation ============

  /**
   * List users who have access to an object
   */
  listUsers(storeId, object, relation, userFilters, context = null, authModelId = null) {
    const url = `${this.baseUrl}/stores/${storeId}/list-users`;
    const payload = {
      object: { type: object.type, id: object.id },
      relation,
      user_filters: userFilters,
    };
    if (context) {
      payload.context = context;
    }
    if (authModelId) {
      payload.authorization_model_id = authModelId;
    }
    const res = http.post(url, JSON.stringify(payload), {
      headers: JSON_HEADERS,
      tags: { endpoint: 'list_users' }
    });
    return this._parseResponse(res);
  }

  // ============ Utility Methods ============

  /**
   * Parse HTTP response and extract JSON body
   */
  _parseResponse(res) {
    let body = null;
    try {
      body = JSON.parse(res.body);
    } catch (e) {
      body = res.body;
    }
    return {
      status: res.status,
      body,
      duration: res.timings.duration,
      success: res.status >= 200 && res.status < 300,
    };
  }

  /**
   * Perform standard checks on a response
   */
  checkResponse(res, name, expectedStatus = 200) {
    return check(res, {
      [`${name} status is ${expectedStatus}`]: (r) => r.status === expectedStatus,
      [`${name} has body`]: (r) => r.body !== null,
    });
  }
}

/**
 * Create a new client instance
 */
export function createClient(baseUrl) {
  return new RsfgaClient(baseUrl);
}

export default RsfgaClient;
