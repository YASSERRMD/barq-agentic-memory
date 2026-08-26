/** Runtime types for client.mjs — the shapes returned by the API. */

export interface MemoryView {
  id: string
  type: 'working' | 'episodic' | 'semantic' | 'procedural' | 'prospective' | string
  text: string
  subject?: string
  status: string
  version: number
  confidence: number
  created_at: string
  updated_at: string
}

export interface ScoredMemory extends MemoryView {
  score: number
}

export declare class BarqError extends Error {
  status: number
}

export declare class Memory {
  constructor(baseUrl: string)
  remember(text: string, options?: {
    tenantId?: string
    userId?: string
    memoryType?: string
    confidence?: number
  }): Promise<MemoryView>
  get(id: string): Promise<MemoryView | null>
  recall(query: string, tenantId?: string | null, limit?: number): Promise<ScoredMemory[]>
  search(query: string, tenantId?: string | null, limit?: number): Promise<MemoryView[]>
  update(id: string, newText: string): Promise<MemoryView>
  forget(id: string, hard?: boolean): Promise<void>
  history(id: string): Promise<MemoryView[]>
}
