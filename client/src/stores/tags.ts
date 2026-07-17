import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, apiVoid } from '../api'
import { useListSync } from '../composables/useListSync'
import type { Tag } from '../types'

export const useTagsStore = defineStore('tags', () => {
  const items = ref<Tag[]>([])
  useListSync('tags', items)

  async function load() {
    items.value = await api<Tag[]>('/api/tags')
  }

  async function create(input: { name: string; description: string | null }) {
    const created = await api<Tag>('/api/tags', {
      method: 'POST',
      body: JSON.stringify(input),
    })
    items.value.push(created)
  }

  async function update(id: number, input: { name: string; description: string | null }) {
    const updated = await api<Tag>(`/api/tags/${id}`, {
      method: 'PUT',
      body: JSON.stringify(input),
    })
    const idx = items.value.findIndex((t) => t.id === id)
    if (idx !== -1) items.value[idx] = updated
  }

  async function remove(id: number) {
    await apiVoid(`/api/tags/${id}`, { method: 'DELETE' })
    items.value = items.value.filter((t) => t.id !== id)
  }

  return { items, load, create, update, remove }
})
