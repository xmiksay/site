import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useMenuStore } from './menu'
import { api, apiVoid } from '../api'
import type { MenuItem } from '../types'

vi.mock('../api', async (importActual) => {
  const actual = await importActual<typeof import('../api')>()
  return { ...actual, api: vi.fn(), apiVoid: vi.fn() }
})

const apiMock = vi.mocked(api)
const apiVoidMock = vi.mocked(apiVoid)

function item(id: number, order_index: number): MenuItem {
  return { id, title: `item-${id}`, path: `p${id}`, markdown: '', order_index, private: false }
}

describe('menu store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('load populates items from the API', async () => {
    apiMock.mockResolvedValueOnce([item(1, 0)])
    const store = useMenuStore()
    await store.load()
    expect(store.items).toHaveLength(1)
  })

  it('create appends the new item and keeps items sorted by order_index', async () => {
    const store = useMenuStore()
    store.items = [item(1, 0), item(2, 10)]
    apiMock.mockResolvedValueOnce(item(3, 5))

    await store.create({ title: 'item-3', path: 'p3', markdown: '', order_index: 5, private: false })

    expect(store.items.map((m) => m.id)).toEqual([1, 3, 2])
  })

  it('update replaces the item in place and re-sorts', async () => {
    const store = useMenuStore()
    store.items = [item(1, 0), item(2, 10)]
    apiMock.mockResolvedValueOnce(item(1, 20))

    await store.update(1, { title: 'item-1', path: 'p1', markdown: '', order_index: 20, private: false })

    expect(store.items.map((m) => m.id)).toEqual([2, 1])
  })

  it('remove deletes on the server then drops the item locally', async () => {
    const store = useMenuStore()
    store.items = [item(1, 0), item(2, 10)]
    apiVoidMock.mockResolvedValueOnce(undefined)
    await store.remove(1)
    expect(apiVoidMock).toHaveBeenCalledWith('/api/menu/1', { method: 'DELETE' })
    expect(store.items.map((m) => m.id)).toEqual([2])
  })
})
