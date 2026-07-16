import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useAuthStore } from './auth'
import { api, apiVoid, ApiError } from '../api'

// Keep the real ApiError (checkSession branches on `instanceof ApiError`);
// only the network calls are mocked.
vi.mock('../api', async (importActual) => {
  const actual = await importActual<typeof import('../api')>()
  return { ...actual, api: vi.fn(), apiVoid: vi.fn() }
})

const apiMock = vi.mocked(api)
const apiVoidMock = vi.mocked(apiVoid)

describe('auth store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('is logged out before any check', () => {
    const store = useAuthStore()
    expect(store.isLoggedIn).toBe(false)
    expect(store.checked).toBe(false)
  })

  it('checkSession sets the user and marks checked', async () => {
    apiMock.mockResolvedValueOnce({ user_id: 1, username: 'martin' })
    const store = useAuthStore()
    await store.checkSession()
    expect(store.isLoggedIn).toBe(true)
    expect(store.user).toEqual({ user_id: 1, username: 'martin' })
    expect(store.checked).toBe(true)
  })

  it('checkSession treats 401 as logged-out, not an error', async () => {
    apiMock.mockRejectedValueOnce(new ApiError(401, 'unauthorized'))
    const store = useAuthStore()
    await expect(store.checkSession()).resolves.toBeUndefined()
    expect(store.isLoggedIn).toBe(false)
    expect(store.checked).toBe(true)
  })

  it('checkSession rethrows non-401 errors but still marks checked', async () => {
    apiMock.mockRejectedValueOnce(new ApiError(500, 'boom'))
    const store = useAuthStore()
    await expect(store.checkSession()).rejects.toThrow('boom')
    expect(store.checked).toBe(true)
  })

  it('logout clears the user', async () => {
    apiMock.mockResolvedValueOnce({ user_id: 1, username: 'martin' })
    apiVoidMock.mockResolvedValueOnce(undefined)
    const store = useAuthStore()
    await store.checkSession()
    await store.logout()
    expect(store.isLoggedIn).toBe(false)
  })
})
