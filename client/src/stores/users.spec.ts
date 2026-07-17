import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useUsersStore } from './users'
import { api, apiVoid } from '../api'
import type { UserSummary } from '../types'

vi.mock('../api', async (importActual) => {
  const actual = await importActual<typeof import('../api')>()
  return { ...actual, api: vi.fn(), apiVoid: vi.fn() }
})

const apiMock = vi.mocked(api)
const apiVoidMock = vi.mocked(apiVoid)

function user(id: number, username: string, is_self = false): UserSummary {
  return { id, username, is_self }
}

describe('users store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('load populates items from the API', async () => {
    apiMock.mockResolvedValueOnce([user(1, 'martin', true)])
    const store = useUsersStore()
    await store.load()
    expect(store.items).toEqual([user(1, 'martin', true)])
  })

  it('create posts credentials and appends the created user', async () => {
    const store = useUsersStore()
    store.items = [user(1, 'martin', true)]
    apiMock.mockResolvedValueOnce(user(2, 'bob'))

    const created = await store.create('bob', 'hunter2')

    expect(apiMock).toHaveBeenCalledWith('/api/users', {
      method: 'POST',
      body: JSON.stringify({ username: 'bob', password: 'hunter2' }),
    })
    expect(created).toEqual(user(2, 'bob'))
    expect(store.items.map((u) => u.id)).toEqual([1, 2])
  })

  it('changePassword calls the API without touching local items', async () => {
    const store = useUsersStore()
    store.items = [user(1, 'martin', true)]
    apiVoidMock.mockResolvedValueOnce(undefined)

    await store.changePassword(1, 'newpass')

    expect(apiVoidMock).toHaveBeenCalledWith('/api/users/1/password', {
      method: 'PUT',
      body: JSON.stringify({ password: 'newpass' }),
    })
    expect(store.items).toEqual([user(1, 'martin', true)])
  })

  it('remove deletes on the server then drops the user locally', async () => {
    const store = useUsersStore()
    store.items = [user(1, 'martin', true), user(2, 'bob')]
    apiVoidMock.mockResolvedValueOnce(undefined)

    await store.remove(2)

    expect(apiVoidMock).toHaveBeenCalledWith('/api/users/2', { method: 'DELETE' })
    expect(store.items.map((u) => u.id)).toEqual([1])
  })
})
