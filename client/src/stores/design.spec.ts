import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useDesignStore } from './design'
import { api, apiBlob, ApiError } from '../api'
import type { DesignState, WsEnvelope } from '../types'

let wsHandler: ((envelope: WsEnvelope) => void) | undefined
vi.mock('./ws', () => ({
  useWsStore: () => ({
    on: (_topic: string, handler: (envelope: WsEnvelope) => void) => {
      wsHandler = handler
      return () => {}
    },
  }),
}))

vi.mock('../api', async (importActual) => {
  const actual = await importActual<typeof import('../api')>()
  return { ...actual, api: vi.fn(), apiBlob: vi.fn() }
})

const apiMock = vi.mocked(api)
const apiBlobMock = vi.mocked(apiBlob)

function designState(overrides: Partial<DesignState> = {}): DesignState {
  return {
    storage: 's3',
    local_dir: false,
    last_reload: null,
    files: [{ path: 'templates/base.html', baked: true, overridden: false, size: 10 }],
    initialized: true,
    changes: [],
    ...overrides,
  }
}

describe('design store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('load fetches the design state', async () => {
    apiMock.mockResolvedValueOnce(designState())
    const store = useDesignStore()
    await store.load()
    expect(apiMock).toHaveBeenCalledWith('/api/design/draft')
    expect(store.files).toHaveLength(1)
  })

  it('save PUTs raw bytes to the encoded path and adopts the fresh state', async () => {
    const store = useDesignStore()
    store.state = designState()
    const fresh = designState({
      files: [{ path: 'templates/my page.html', baked: false, overridden: true, size: 3 }],
    })
    apiMock.mockResolvedValueOnce(fresh)

    await store.save('templates/my page.html', 'abc')

    const [url, init] = apiMock.mock.calls[0]
    expect(url).toBe('/api/design/draft/templates/my%20page.html')
    expect(init).toMatchObject({ method: 'PUT', body: 'abc' })
    expect(new Headers(init!.headers).get('Content-Type')).toBe('application/octet-stream')
    expect(store.state).toEqual(fresh)
  })

  it('save rejected with 422 leaves the state untouched and rethrows', async () => {
    const store = useDesignStore()
    const before = designState()
    store.state = before
    apiMock.mockRejectedValueOnce(new ApiError(422, 'templates/base.html: syntax error'))

    const err = await store.save('templates/base.html', '{% bad').catch((e) => e)
    expect(err).toBeInstanceOf(ApiError)
    expect((err as ApiError).status).toBe(422)
    expect(store.state).toStrictEqual(before)
  })

  it('remove DELETEs the override and adopts the fresh state', async () => {
    const store = useDesignStore()
    store.state = designState({
      files: [{ path: 'assets/css/x.css', baked: false, overridden: true, size: 1 }],
    })
    apiMock.mockResolvedValueOnce(designState({ files: [] }))

    await store.remove('assets/css/x.css')
    expect(apiMock).toHaveBeenCalledWith('/api/design/draft/assets/css/x.css', { method: 'DELETE' })
    expect(store.files).toHaveLength(0)
  })

  it('reload POSTs and adopts the returned state', async () => {
    const store = useDesignStore()
    const fresh = designState({
      last_reload: { at: '2026-10-09T18:00:00Z', ok: true, files: 12, error: null, completed_publish: null },
    })
    apiMock.mockResolvedValueOnce(fresh)
    await store.reload()
    expect(apiMock).toHaveBeenCalledWith('/api/design/reload', { method: 'POST' })
    expect(store.state?.last_reload?.files).toBe(12)
  })

  it('a failed reload refreshes the state to pick up last_reload, then rethrows', async () => {
    const store = useDesignStore()
    const failed = designState({
      last_reload: { at: '2026-10-09T18:00:00Z', ok: false, files: 0, error: 'bucket down', completed_publish: null },
    })
    apiMock.mockRejectedValueOnce(new ApiError(503, 'storage unavailable'))
    apiMock.mockResolvedValueOnce(failed)

    await expect(store.reload()).rejects.toThrow('storage unavailable')
    expect(store.state?.last_reload?.error).toBe('bucket down')
  })

  it('publish POSTs, reloads the draft state and records the version', async () => {
    const entry = { id: '2026-10-10T12:00:00.000000Z', at: '2026-10-10T12:00:00Z', by: 'me', files: 3 }
    apiMock.mockResolvedValueOnce(entry)
    apiMock.mockResolvedValueOnce(designState())
    const store = useDesignStore()
    const revision = store.revision

    expect(await store.publish()).toEqual({ kind: 'published', entry })
    expect(apiMock).toHaveBeenNthCalledWith(1, '/api/design/publish', { method: 'POST' })
    expect(apiMock).toHaveBeenNthCalledWith(2, '/api/design/draft')
    expect(store.history[0]).toEqual(entry)
    expect(store.revision).toBe(revision + 1)
  })

  it('publish(true) forces past a conflict', async () => {
    apiMock.mockResolvedValueOnce({ id: 'x', at: 'x', by: 'me', files: 1 })
    apiMock.mockResolvedValueOnce(designState())
    await useDesignStore().publish(true)
    expect(apiMock).toHaveBeenNthCalledWith(1, '/api/design/publish?force=true', { method: 'POST' })
  })

  it('a 409 conflict resolves with the paths and keeps the state', async () => {
    const store = useDesignStore()
    const before = designState()
    store.state = before
    apiMock.mockRejectedValueOnce(
      new ApiError(
        409,
        'design/ changed outside the draft since it was started: templates/base.html; discard the draft to adopt those changes, or publish with force=true to overwrite them',
      ),
    )
    expect(await store.publish()).toMatchObject({ kind: 'conflict', paths: ['templates/base.html'] })
    expect(apiMock).toHaveBeenCalledTimes(1)
    expect(store.state).toStrictEqual(before)
  })

  it('a 409 with nothing to publish resolves as "nothing"', async () => {
    apiMock.mockRejectedValueOnce(new ApiError(409, 'nothing to publish: the draft matches the live design'))
    expect(await useDesignStore().publish()).toMatchObject({ kind: 'nothing' })
  })

  it('a 422 resolves with one error per template problem', async () => {
    apiMock.mockRejectedValueOnce(
      new ApiError(422, 'templates/a.html:2: undefined value (rendering 404); templates/b.html: syntax error'),
    )
    expect(await useDesignStore().publish()).toEqual({
      kind: 'invalid',
      errors: ['templates/a.html:2: undefined value (rendering 404)', 'templates/b.html: syntax error'],
    })
  })

  it('other publish failures reject', async () => {
    apiMock.mockRejectedValueOnce(new ApiError(503, 'storage unavailable'))
    await expect(useDesignStore().publish()).rejects.toThrow('storage unavailable')
  })

  it('discard POSTs and adopts the returned state', async () => {
    const store = useDesignStore()
    store.state = designState({ changes: [{ path: 'assets/a.css', kind: 'added' }] })
    apiMock.mockResolvedValueOnce(designState())
    await store.discard()
    expect(apiMock).toHaveBeenCalledWith('/api/design/draft/discard', { method: 'POST' })
    expect(store.state?.changes).toEqual([])
  })

  it('fetchText requests the baked source when asked', async () => {
    apiBlobMock.mockResolvedValueOnce({ blob: new Blob(['hello']), filename: 'base.html' })
    const store = useDesignStore()
    const text = await store.fetchText('templates/base.html', 'baked')
    expect(apiBlobMock).toHaveBeenCalledWith('/api/design/draft/templates/base.html?source=baked')
    expect(text).toBe('hello')
  })

  it('loadHistory lists the versions; restore POSTs the encoded id and adopts the draft', async () => {
    const entries = [{ id: '2026-10-10T12:00:00Z', at: '2026-10-10T12:00:00Z', by: 'me', files: 3 }]
    apiMock.mockResolvedValueOnce(entries)
    const store = useDesignStore()
    await store.loadHistory()
    expect(apiMock).toHaveBeenCalledWith('/api/design/history')
    expect(store.history).toEqual(entries)

    const restored = designState({ changes: [{ path: 'templates/base.html', kind: 'modified' }] })
    apiMock.mockResolvedValueOnce(restored)
    const revision = store.revision
    await store.restore('2026-10-10T12:00:00Z')
    expect(apiMock).toHaveBeenLastCalledWith('/api/design/history/2026-10-10T12%3A00%3A00Z/restore', {
      method: 'POST',
    })
    expect(store.state).toEqual(restored)
    expect(store.revision).toBe(revision + 1)
  })

  it('setPreview POSTs the toggle', async () => {
    apiMock.mockResolvedValueOnce({ on: true })
    await useDesignStore().setPreview(true)
    expect(apiMock).toHaveBeenCalledWith('/api/design/preview', {
      method: 'POST',
      body: JSON.stringify({ on: true }),
    })
  })

  it('a design WS event bumps the revision and refreshes a loaded draft', async () => {
    const store = useDesignStore()
    wsHandler!({ topic: 'design', event: 'draft_changed', payload: { action: 'put', path: 'a' } })
    expect(store.revision).toBe(1)
    expect(apiMock).not.toHaveBeenCalled()

    store.state = designState()
    apiMock.mockResolvedValueOnce(designState({ changes: [{ path: 'templates/base.html', kind: 'modified' }] }))
    wsHandler!({ topic: 'design', event: 'draft_changed', payload: { action: 'put', path: 'a' } })
    await vi.waitFor(() => expect(store.state?.changes).toHaveLength(1))
    expect(store.revision).toBe(2)
    expect(apiMock).toHaveBeenCalledWith('/api/design/draft')
  })

  it('a published WS event also refreshes a loaded history', async () => {
    const store = useDesignStore()
    store.state = designState()
    store.history = [{ id: 'old', at: 'x', by: 'me', files: 1 }]
    const fresh = [{ id: 'new', at: 'y', by: 'ai-admin', files: 2 }, ...store.history]
    apiMock.mockImplementation(async (url: string) =>
      (url === '/api/design/history' ? fresh : designState()) as never,
    )
    wsHandler!({ topic: 'design', event: 'published', payload: fresh[0] })
    await vi.waitFor(() => expect(store.history[0].id).toBe('new'))
    apiMock.mockReset()
  })
})
