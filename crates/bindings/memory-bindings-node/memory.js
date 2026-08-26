// The blueprint's Memory class, wrapping the raw napi function exports
// that `npx napi build` generates into index.js.
const native = require('./index.js')

/** Maps a canonical record's serde names onto the public TS shape. */
function shapeRecord(raw) {
  if (Array.isArray(raw)) return raw.map(shapeRecord)
  return {
    id: raw.id,
    type: raw.memory_type,
    text: raw.content?.text ?? '',
    subject: raw.subject,
    status: raw.status,
    version: raw.version,
    created_at: raw.created_at,
    confidence: raw.confidence,
  }
}

class Memory {
  /**
   * @param {string|null} path File-backed persistence (null = in-memory)
   * @param {string} [namespace] Logical namespace
   */
  constructor(path = null, namespace = 'default') {
    this._handle = native.memoryOpen(path, namespace)
  }

  close() {
    native.memoryClose(this._handle)
  }

  /** @param {{content: string, memoryType?: string, tenantId?: string,
   *           userId?: string, agentId?: string, sessionId?: string,
   *           confidence?: number}} options */
  remember(options) {
    return shapeRecord(JSON.parse(
      native.memoryRemember(this._handle, JSON.stringify({
        text: options.content,
        type: options.memoryType,
        tenant_id: options.tenantId,
        user_id: options.userId,
        agent_id: options.agentId,
        session_id: options.sessionId,
        confidence: options.confidence ?? 0.5,
      })),
    ))
  }

  search(query, tenantId = null, userId = null, limit = 10) {
    return shapeRecord(JSON.parse(native.memorySearch(this._handle, JSON.stringify({
      query, tenant_id: tenantId, user_id: userId, limit,
    }))))
  }

  /** Hybrid recall: semantic when available, keyword fallback. */
  recall(query, tenantId = null, userId = null, limit = 10) {
    return shapeRecord(JSON.parse(native.memoryRecall(this._handle, JSON.stringify({
      query, tenant_id: tenantId, user_id: userId, limit,
    }))))
  }

  update(id, newText) {
    return shapeRecord(JSON.parse(native.memoryUpdate(this._handle, JSON.stringify({ id, text: newText }))))
  }

  forget(id) {
    return native.memoryForget(this._handle, id)
  }

  history(id) {
    return shapeRecord(JSON.parse(native.memoryHistory(this._handle, id)))
  }
}

module.exports = { Memory }
