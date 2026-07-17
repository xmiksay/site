import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useGalleriesStore } from './galleries'
import { api, apiVoid } from '../api'
import type { Gallery, WsEnvelope } from '../types'

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

function gallery(id: number, path: string): Gallery {
  return {
    id,
    path,
    title: path,
    description: null,
    file_ids: [],
    created_at: '2024-01-01',
    created_by: 1,
  }
}

describe('galleries store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    wsHandler = undefined
  })

  it('load populates items from the API', async () => {
    apiMock.mockResolvedValueOnce([gallery(1, 'a')])
    const store = useGalleriesStore()
    await store.load()
    expect(store.items).toHaveLength(1)
  })

  it('loadPaths returns the raw path list', async () => {
    apiMock.mockResolvedValueOnce(['a', 'b'])
    const store = useGalleriesStore()
    await expect(store.loadPaths()).resolves.toEqual(['a', 'b'])
    expect(apiMock).toHaveBeenCalledWith('/api/galleries/paths')
  })

  it('create prepends the new gallery', async () => {
    const store = useGalleriesStore()
    store.items = [gallery(1, 'existing')]
    apiMock.mockResolvedValueOnce(gallery(2, 'new'))
    const created = await store.create({ path: 'new', title: 'New', description: null, file_ids: [] })
    expect(created.id).toBe(2)
    expect(store.items.map((g) => g.id)).toEqual([2, 1])
  })

  it('update replaces the matching gallery in place', async () => {
    const store = useGalleriesStore()
    store.items = [gallery(1, 'old')]
    apiMock.mockResolvedValueOnce(gallery(1, 'renamed'))
    const updated = await store.update(1, {
      path: 'renamed',
      title: 'Renamed',
      description: null,
      file_ids: [],
    })
    expect(updated.path).toBe('renamed')
    expect(store.items[0].path).toBe('renamed')
  })

  it('update is a no-op on items when the id is not found locally', async () => {
    const store = useGalleriesStore()
    store.items = [gallery(1, 'old')]
    apiMock.mockResolvedValueOnce(gallery(2, 'other'))
    await store.update(2, { path: 'other', title: 'x', description: null, file_ids: [] })
    expect(store.items).toEqual([gallery(1, 'old')])
  })

  it('remove deletes on the server then drops the item locally', async () => {
    const store = useGalleriesStore()
    store.items = [gallery(1, 'a'), gallery(2, 'b')]
    apiVoidMock.mockResolvedValueOnce(undefined)
    await store.remove(1)
    expect(store.items.map((g) => g.id)).toEqual([2])
  })

  it('a ws upsert envelope for the galleries topic upserts by id', () => {
    const store = useGalleriesStore()
    store.items = [gallery(1, 'a')]
    expect(wsHandler).toBeDefined()
    wsHandler!({ topic: 'galleries', event: 'updated', payload: gallery(1, 'a-renamed') })
    expect(store.items).toEqual([gallery(1, 'a-renamed')])
  })
})
