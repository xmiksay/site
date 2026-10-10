import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useDesignStore } from './design'
import { api, apiBlob, ApiError } from '../api'
import type { DesignState } from '../types'

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
      last_reload: { at: '2026-10-09T18:00:00Z', ok: true, files: 12, error: null },
    })
    apiMock.mockResolvedValueOnce(fresh)
    await store.reload()
    expect(apiMock).toHaveBeenCalledWith('/api/design/reload', { method: 'POST' })
    expect(store.state?.last_reload?.files).toBe(12)
  })

  it('a failed reload refreshes the state to pick up last_reload, then rethrows', async () => {
    const store = useDesignStore()
    const failed = designState({
      last_reload: { at: '2026-10-09T18:00:00Z', ok: false, files: 0, error: 'bucket down' },
    })
    apiMock.mockRejectedValueOnce(new ApiError(503, 'storage unavailable'))
    apiMock.mockResolvedValueOnce(failed)

    await expect(store.reload()).rejects.toThrow('storage unavailable')
    expect(store.state?.last_reload?.error).toBe('bucket down')
  })

  it('publish POSTs, then reloads the draft state', async () => {
    const entry = { id: '2026-10-10T12:00:00.000000Z', at: '2026-10-10T12:00:00Z', by: 'me', files: 3 }
    apiMock.mockResolvedValueOnce(entry)
    apiMock.mockResolvedValueOnce(designState())
    const store = useDesignStore()

    expect(await store.publish()).toEqual(entry)
    expect(apiMock).toHaveBeenNthCalledWith(1, '/api/design/publish', { method: 'POST' })
    expect(apiMock).toHaveBeenNthCalledWith(2, '/api/design/draft')
  })

  it('publish(true) forces past a conflict', async () => {
    apiMock.mockResolvedValueOnce({ id: 'x', at: 'x', by: 'me', files: 1 })
    apiMock.mockResolvedValueOnce(designState())
    await useDesignStore().publish(true)
    expect(apiMock).toHaveBeenNthCalledWith(1, '/api/design/publish?force=true', { method: 'POST' })
  })

  it('a rejected publish (409) rethrows and keeps the state', async () => {
    const store = useDesignStore()
    const before = designState()
    store.state = before
    apiMock.mockRejectedValueOnce(new ApiError(409, 'design/ changed outside the draft'))
    await expect(store.publish()).rejects.toThrow('changed outside the draft')
    expect(store.state).toStrictEqual(before)
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
    const text = await store.fetchText('templates/base.html', true)
    expect(apiBlobMock).toHaveBeenCalledWith('/api/design/draft/templates/base.html?source=baked')
    expect(text).toBe('hello')
  })
})
