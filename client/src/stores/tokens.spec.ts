import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useTokensStore } from './tokens'
import { api, apiVoid } from '../api'

vi.mock('../api', async (importActual) => {
  const actual = await importActual<typeof import('../api')>()
  return { ...actual, api: vi.fn(), apiVoid: vi.fn() }
})

const apiMock = vi.mocked(api)
const apiVoidMock = vi.mocked(apiVoid)

describe('tokens store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('load populates items from the API', async () => {
    apiMock.mockResolvedValueOnce([
      { id: 1, label: 'ci', is_service: true, expires_at: null },
    ])
    const store = useTokensStore()
    await store.load()
    expect(store.items).toHaveLength(1)
    expect(store.items[0].label).toBe('ci')
  })

  it('create optimistically prepends the new token summary', async () => {
    const store = useTokensStore()
    store.items = [{ id: 1, label: 'old', is_service: false, expires_at: null }]
    apiMock.mockResolvedValueOnce({
      id: 2,
      label: 'new',
      is_service: true,
      expires_at: null,
      nonce: 'secret',
    })
    const created = await store.create('new')
    expect(created.nonce).toBe('secret')
    expect(store.items.map((t) => t.id)).toEqual([2, 1])
  })

  it('remove drops the token by id', async () => {
    const store = useTokensStore()
    store.items = [
      { id: 1, label: 'a', is_service: false, expires_at: null },
      { id: 2, label: 'b', is_service: false, expires_at: null },
    ]
    apiVoidMock.mockResolvedValueOnce(undefined)
    await store.remove(1)
    expect(store.items.map((t) => t.id)).toEqual([2])
  })
})
