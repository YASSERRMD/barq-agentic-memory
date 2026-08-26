import { createRequire } from 'node:module'
import pkg from './memory.js'
const { Memory } = pkg

// 1. In-memory engine — the blueprint experience.
const mem = new Memory(null, 'smoke')
const saved = mem.remember({ content: 'Customer prefers email.', userId: '123', tenantId: 'acme' })
if (!saved.id || saved.text !== 'Customer prefers email.') throw new Error('remember failed')

const hits = mem.recall('How should I contact this customer?', 'acme', '123', 5)
if (hits.length < 1 || !hits[0].text.includes('email')) throw new Error('recall failed')

// 2. Restart persistence via file store.
const os = await import('node:os'), path = await import('node:path'), fs = await import('node:fs')
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'barq-node-'))
const db = path.join(dir, 'mem.redb')
const m1 = new Memory(db, 'n1')
const fact = m1.remember({ content: 'Project Atlas uses PostgreSQL' })

const m2 = new Memory(db, 'n1') // "restart"
const found = m2.search('atlas postgresql', null, null, 10)
if (!found.some((h) => h.id === fact.id)) throw new Error('persistence failed')

// 3. Update/history/forget.
const newer = m2.update(fact.id, 'Atlas migrated to MySQL')
const chain = m2.history(newer.id)
if (chain.length !== 2) throw new Error(`history chain ${chain.length} != 2`)
if (m2.forget(newer.id) !== true) throw new Error('forget returned false')
if (m2.search('atlas mysql', null, null, 10).some((h) => h.id === newer.id)) {
  throw new Error('forgotten memory still searchable')
}
m2.close()

console.log('NODE BINDING SMOKE TEST OK')
