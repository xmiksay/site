import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, apiVoid } from '../api'
import { useListSync } from '../composables/useListSync'
import type { FileSummary } from '../types'

export const useFilesStore = defineStore('files', () => {
  const items = ref<FileSummary[]>([])
  useListSync('files', items)

  async function load(mimePrefix?: string) {
    const qs = mimePrefix ? `?mime_prefix=${encodeURIComponent(mimePrefix)}` : ''
    items.value = await api<FileSummary[]>(`/api/files${qs}`)
  }

  async function read(id: number) {
    return await api<FileSummary>(`/api/files/${id}`)
  }

  async function upload(file: File, path: string, description: string | null): Promise<FileSummary> {
    const fd = new FormData()
    fd.append('file', file)
    fd.append('path', path)
    if (description) fd.append('description', description)
    const created = await api<FileSummary>('/api/files', { method: 'POST', body: fd })
    items.value.unshift(created)
    return created
  }

  async function update(id: number, input: { path: string; description: string | null }) {
    const updated = await api<FileSummary>(`/api/files/${id}`, {
      method: 'PUT',
      body: JSON.stringify(input),
    })
    const idx = items.value.findIndex((f) => f.id === id)
    if (idx !== -1) items.value[idx] = updated
    return updated
  }

  async function remove(id: number) {
    await apiVoid(`/api/files/${id}`, { method: 'DELETE' })
    items.value = items.value.filter((f) => f.id !== id)
  }

  return { items, load, read, upload, update, remove }
})
