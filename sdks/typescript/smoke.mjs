import { Memory } from './client.mjs'

const base = process.env.BARQ_BASE ?? 'http://127.0.0.1:18099'
const client = new Memory(base)

const saved = await client.remember('TypeScript SDK smoke fact', { tenantId: 'acme' })
if (!saved.id) throw new Error('remember failed')

const hits = await client.recall('sdk smoke fact', 'acme', 5)
if (!hits.some((h) => h.id === saved.id)) throw new Error('recall failed')

const successor = await client.update(saved.id, 'TypeScript SDK smoke fact v2')
const chain = await client.history(successor.id)
if (chain.length !== 2) throw new Error(`history ${chain.length} != 2`)

await client.forget(successor.id)
if ((await client.get(successor.id)) !== null) throw new Error('forgotten still visible')

console.log('TYPESCRIPT SDK SMOKE TEST OK')
