import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useFilesStore } from './files'
import { api, apiVoid } from '../api'
import type { FileSummary, WsEnvelope } from '../types'

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

function file(id: number, path: string): FileSummary {
  return {
    id,
    hash: `hash-${id}`,
    path,
    title: path,
    description: null,
    mimetype: 'image/png',
    size_bytes: 100,
    has_thumbnail: true,
    created_at: '2024-01-01',
  }
}

describe('files store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    wsHandler = undefined
  })

  it('load with no mime filter hits the plain endpoint', async () => {
    apiMock.mockResolvedValueOnce([file(1, 'a.png')])
    const store = useFilesStore()
    await store.load()
    expect(apiMock).toHaveBeenCalledWith('/api/files')
    expect(store.items).toHaveLength(1)
  })

  it('load with a mime filter encodes it into the query string', async () => {
    apiMock.mockResolvedValueOnce([])
    const store = useFilesStore()
    await store.load('image/')
    expect(apiMock).toHaveBeenCalledWith('/api/files?mime_prefix=image%2F')
  })

  it('upload builds a FormData body and prepends the created file', async () => {
    const store = useFilesStore()
    store.items = [file(1, 'existing.png')]
    apiMock.mockResolvedValueOnce(file(2, 'new.png'))

    const blob = new File(['x'], 'new.png', { type: 'image/png' })
    const created = await store.upload(blob, 'new.png', 'a description')

    expect(created.id).toBe(2)
    expect(store.items.map((f) => f.id)).toEqual([2, 1])

    const [, init] = apiMock.mock.calls[0]
    const fd = init!.body as FormData
    expect(fd.get('path')).toBe('new.png')
    expect(fd.get('description')).toBe('a description')
    expect(fd.get('file')).toBe(blob)
  })

  it('upload omits the description field when none is given', async () => {
    const store = useFilesStore()
    apiMock.mockResolvedValueOnce(file(1, 'new.png'))
    await store.upload(new File(['x'], 'new.png'), 'new.png', null)

    const [, init] = apiMock.mock.calls[0]
    const fd = init!.body as FormData
    expect(fd.has('description')).toBe(false)
  })

  it('update replaces the matching item in place', async () => {
    const store = useFilesStore()
    store.items = [file(1, 'old.png')]
    apiMock.mockResolvedValueOnce(file(1, 'renamed.png'))

    const updated = await store.update(1, { path: 'renamed.png', description: null })
    expect(updated.path).toBe('renamed.png')
    expect(store.items[0].path).toBe('renamed.png')
  })

  it('remove deletes on the server then drops the item locally', async () => {
    const store = useFilesStore()
    store.items = [file(1, 'a.png'), file(2, 'b.png')]
    apiVoidMock.mockResolvedValueOnce(undefined)
    await store.remove(1)
    expect(store.items.map((f) => f.id)).toEqual([2])
  })

  it('a ws deleted envelope removes the item by id', () => {
    const store = useFilesStore()
    store.items = [file(1, 'a.png')]
    expect(wsHandler).toBeDefined()
    wsHandler!({ topic: 'files', event: 'deleted', payload: { id: 1 } })
    expect(store.items).toHaveLength(0)
  })
})
