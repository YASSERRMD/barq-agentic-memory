export interface RememberOptions {
  content: string
  memoryType?: 'working' | 'episodic' | 'semantic' | 'procedural' | 'prospective'
  tenantId?: string
  userId?: string
  agentId?: string
  sessionId?: string
  confidence?: number
}

export interface MemoryRecord {
  id: string
  type: 'working' | 'episodic' | 'semantic' | 'procedural' | 'prospective'
  text: string
  subject?: string
  status: string
  version: number
  created_at: string
  confidence: number
}

export declare class Memory {
  constructor(path?: string | null, namespace?: string)
  remember(options: RememberOptions): MemoryRecord
  search(query: string, tenantId?: string | null, userId?: string | null, limit?: number): MemoryRecord[]
  recall(query: string, tenantId?: string | null, userId?: string | null, limit?: number): MemoryRecord[]
  update(id: string, newText: string): MemoryRecord
  forget(id: string): boolean
  history(id: string): MemoryRecord[]
}
