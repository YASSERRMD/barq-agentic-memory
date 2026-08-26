/**
 * Barq memory engine client for TypeScript/Node (fetch-based, no deps).
 * Mirrors the Rust/Python/.NET SDKs concept-for-concept.
 * Runtime types are documented in client.d.mts.
 */

export class BarqError extends Error {
  /**
   * @param {number} status
   * @param {string} message
   */
  constructor(status, message) {
    super(`api (${status}): ${message}`)
    this.name = 'BarqError'
    this.status = status
  }
}

export class Memory {
  /**
   * @param {string} baseUrl
   */
  constructor(baseUrl) {
    this.base = baseUrl.replace(/\/$/, '')
  }

  /**
   * @param {string} method
   * @param {string} path
   * @param {unknown} [body]
   * @returns {Promise<any>}
   */
  async call(method, path, body) {
    const response = await fetch(`${this.base}${path}`, {
      method,
      headers: body !== undefined ? { 'content-type': 'application/json' } : {},
      body: body !== undefined ? JSON.stringify(body) : undefined,
    })
    const text = await response.text()
    const json = text ? JSON.parse(text) : null
    if (!response.ok) {
      throw new BarqError(response.status, json?.message ?? response.statusText)
    }
    return json
  }

  /**
   * @param {string} text
   * @param {{ tenantId?: string, userId?: string, memoryType?: string, confidence?: number }} [options]
   * @returns {Promise<import('./client.d.mts').MemoryView>}
   */
  remember(text, options = {}) {
    return this.call('POST', '/v1/memories', {
      text,
      tenant_id: options.tenantId,
      user_id: options.userId,
      type: options.memoryType,
      confidence: options.confidence,
    })
  }

  /**
   * @param {string} id
   * @returns {Promise<import('./client.d.mts').MemoryView | null>}
   */
  async get(id) {
    try {
      return await this.call('GET', `/v1/memories/${id}`)
    } catch (e) {
      if (e instanceof BarqError && e.status === 404) return null
      throw e
    }
  }

  /**
   * @param {string} query
   * @param {string | null} [tenantId]
   * @param {number} [limit]
   * @returns {Promise<import('./client.d.mts').ScoredMemory[]>}
   */
  recall(query, tenantId = null, limit = 10) {
    return this.call('POST', '/v1/recall', { query, tenant_id: tenantId, limit })
  }

  /**
   * @param {string} query
   * @param {string | null} [tenantId]
   * @param {number} [limit]
   * @returns {Promise<import('./client.d.mts').MemoryView[]>}
   */
  search(query, tenantId = null, limit = 10) {
    return this.call('POST', '/v1/search', { query, tenant_id: tenantId, limit })
  }

  /**
   * @param {string} id
   * @param {string} newText
   * @returns {Promise<import('./client.d.mts').MemoryView>}
   */
  update(id, newText) {
    return this.call('PATCH', `/v1/memories/${id}`, { text: newText })
  }

  /**
   * @param {string} id
   * @param {boolean} [hard]
   */
  async forget(id, hard = false) {
    await this.call('DELETE', `/v1/memories/${id}${hard ? '?hard=true' : ''}`)
  }

  /**
   * @param {string} id
   * @returns {Promise<import('./client.d.mts').MemoryView[]>}
   */
  history(id) {
    return this.call('GET', `/v1/memories/${id}/history`)
  }
}
