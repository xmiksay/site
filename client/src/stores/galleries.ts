import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, apiVoid } from '../api'
import { useListSync } from '../composables/useListSync'
import type { Gallery } from '../types'

export interface GalleryInput {
  path: string
  title: string
  description: string | null
  file_ids: number[]
}

export const useGalleriesStore = defineStore('galleries', () => {
  const items = ref<Gallery[]>([])
  useListSync('galleries', items)

  async function load() {
    items.value = await api<Gallery[]>('/api/galleries')
  }

  async function loadPaths(): Promise<string[]> {
    return await api<string[]>('/api/galleries/paths')
  }

  async function read(id: number) {
    return await api<Gallery>(`/api/galleries/${id}`)
  }

  async function create(input: GalleryInput) {
    const created = await api<Gallery>('/api/galleries', {
      method: 'POST',
      body: JSON.stringify(input),
    })
    items.value.unshift(created)
    return created
  }

  async function update(id: number, input: GalleryInput) {
    const updated = await api<Gallery>(`/api/galleries/${id}`, {
      method: 'PUT',
      body: JSON.stringify(input),
    })
    const idx = items.value.findIndex((g) => g.id === id)
    if (idx !== -1) items.value[idx] = updated
    return updated
  }

  async function remove(id: number) {
    await apiVoid(`/api/galleries/${id}`, { method: 'DELETE' })
    items.value = items.value.filter((g) => g.id !== id)
  }

  return { items, load, loadPaths, read, create, update, remove }
})
