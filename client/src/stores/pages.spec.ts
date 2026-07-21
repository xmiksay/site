import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { usePagesStore } from './pages'
import { api, apiBlob, apiVoid } from '../api'
import type { PageSummary, WsEnvelope } from '../types'

vi.mock('../api', async (importActual) => {
  const actual = await importActual<typeof import('../api')>()
  return { ...actual, api: vi.fn(), apiVoid: vi.fn(), apiBlob: vi.fn() }
})

// `pages.ts` wires `useListSync('pages', items)` at store-creation time, which
// registers a handler via `useWsStore().on(...)` — stub that store the same
// way `assistant.spec.ts` does so tests can push synthetic envelopes and
// exercise the live-sync path (upsert / delete) alongside the REST calls.
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
const apiBlobMock = vi.mocked(apiBlob)

function page(id: number, path: string): PageSummary {
  return {
    id,
    path,
    summary: null,
    tag_ids: [],
    private: false,
    created_at: '2024-01-01',
    modified_at: '2024-01-01',
  }
}

describe('pages store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    wsHandler = undefined
  })

  it('load populates items from the API', async () => {
    apiMock.mockResolvedValueOnce([page(1, 'a')])
    const store = usePagesStore()
    await store.load()
    expect(store.items).toEqual([page(1, 'a')])
  })

  it('loadPaths omits the query string when no args are given', async () => {
    apiMock.mockResolvedValueOnce(['a', 'b'])
    const store = usePagesStore()
    const paths = await store.loadPaths()
    expect(apiMock).toHaveBeenCalledWith('/api/pages/paths')
    expect(paths).toEqual(['a', 'b'])
  })

  it('loadPaths builds a query string from prefix and limit', async () => {
    apiMock.mockResolvedValueOnce(['a'])
    const store = usePagesStore()
    await store.loadPaths('obsidian', 5)
    expect(apiMock).toHaveBeenCalledWith('/api/pages/paths?prefix=obsidian&limit=5')
  })

  it('remove deletes on the server then drops the item locally', async () => {
    const store = usePagesStore()
    store.items = [page(1, 'a'), page(2, 'b')]
    apiVoidMock.mockResolvedValueOnce(undefined)
    await store.remove(1)
    expect(apiVoidMock).toHaveBeenCalledWith('/api/pages/1', { method: 'DELETE' })
    expect(store.items.map((p) => p.id)).toEqual([2])
  })

  it('create and update pass through the API response', async () => {
    const store = usePagesStore()
    const input = { path: 'a', summary: null, markdown: '', tag_ids: [], private: false }
    apiMock.mockResolvedValueOnce(page(1, 'a'))
    const created = await store.create(input)
    expect(created.id).toBe(1)

    apiMock.mockResolvedValueOnce(page(1, 'a-renamed'))
    const updated = await store.update(1, { ...input, path: 'a-renamed' })
    expect(updated.path).toBe('a-renamed')
  })

  it('a ws upsert envelope for the pages topic adds a new item', () => {
    const store = usePagesStore()
    store.items = [page(1, 'a')]
    expect(wsHandler).toBeDefined()

    wsHandler!({ topic: 'pages', event: 'updated', payload: page(2, 'b') })
    expect(store.items.map((p) => p.id)).toEqual([2, 1])
  })

  it('a ws upsert envelope replaces an existing item in place', () => {
    const store = usePagesStore()
    store.items = [page(1, 'a')]

    wsHandler!({ topic: 'pages', event: 'updated', payload: page(1, 'a-renamed') })
    expect(store.items).toEqual([page(1, 'a-renamed')])
  })

  it('a ws deleted envelope removes the item by id', () => {
    const store = usePagesStore()
    store.items = [page(1, 'a'), page(2, 'b')]

    wsHandler!({ topic: 'pages', event: 'deleted', payload: { id: 1 } })
    expect(store.items.map((p) => p.id)).toEqual([2])
  })

  it('exportPage requests the export endpoint and returns the blob + filename', async () => {
    const blob = new Blob(['pdf-bytes'])
    apiBlobMock.mockResolvedValueOnce({ blob, filename: 'my-page.pdf' })
    const store = usePagesStore()
    const result = await store.exportPage(1, 'pdf')
    expect(apiBlobMock).toHaveBeenCalledWith('/api/export/pages/1?format=pdf')
    expect(result).toEqual({ blob, filename: 'my-page.pdf' })
  })
})
