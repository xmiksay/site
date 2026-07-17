import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useTagsStore } from './tags'
import { api, apiVoid } from '../api'
import type { Tag, WsEnvelope } from '../types'

vi.mock('../api', async (importActual) => {
  const actual = await importActual<typeof import('../api')>()
  return { ...actual, api: vi.fn(), apiVoid: vi.fn() }
})

let wsHandler: ((envelope: WsEnvelope) => void) | undefined
vi.mock('./ws', () => ({
  useWsStore: () => ({
    on: (_topic: string, handler: (envelope: WsEnvelope) => void) => {
      wsHandler = handler
      return () => {}
    },
  }),
}))

const apiMock = vi.mocked(api)
const apiVoidMock = vi.mocked(apiVoid)

function tag(id: number, name: string): Tag {
  return { id, name, description: null }
}

describe('tags store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    wsHandler = undefined
  })

  it('load populates items from the API', async () => {
    apiMock.mockResolvedValueOnce([tag(1, 'rust')])
    const store = useTagsStore()
    await store.load()
    expect(store.items).toEqual([tag(1, 'rust')])
  })

  it('create appends the new tag', async () => {
    const store = useTagsStore()
    store.items = [tag(1, 'rust')]
    apiMock.mockResolvedValueOnce(tag(2, 'vue'))
    await store.create({ name: 'vue', description: null })
    expect(store.items.map((t) => t.id)).toEqual([1, 2])
  })

  it('update replaces the matching tag in place', async () => {
    const store = useTagsStore()
    store.items = [tag(1, 'rust')]
    apiMock.mockResolvedValueOnce(tag(1, 'rustlang'))
    await store.update(1, { name: 'rustlang', description: null })
    expect(store.items[0].name).toBe('rustlang')
  })

  it('remove deletes on the server then drops the item locally', async () => {
    const store = useTagsStore()
    store.items = [tag(1, 'rust'), tag(2, 'vue')]
    apiVoidMock.mockResolvedValueOnce(undefined)
    await store.remove(1)
    expect(store.items.map((t) => t.id)).toEqual([2])
  })

  it('a ws deleted envelope removes the tag by id', () => {
    const store = useTagsStore()
    store.items = [tag(1, 'rust')]
    expect(wsHandler).toBeDefined()
    wsHandler!({ topic: 'tags', event: 'deleted', payload: { id: 1 } })
    expect(store.items).toHaveLength(0)
  })
})
