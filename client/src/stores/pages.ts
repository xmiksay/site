import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, apiBlob, apiVoid } from '../api'
import { useListSync } from '../composables/useListSync'
import type { PageDetail, PageInput, PageSummary, RevisionDetail } from '../types'

export const usePagesStore = defineStore('pages', () => {
  const items = ref<PageSummary[]>([])
  useListSync('pages', items)

  async function load() {
    items.value = await api<PageSummary[]>('/api/pages')
  }

  async function loadPaths(prefix?: string, limit?: number): Promise<string[]> {
    const qs = new URLSearchParams()
    if (prefix) qs.set('prefix', prefix)
    if (limit) qs.set('limit', String(limit))
    const suffix = qs.toString() ? `?${qs}` : ''
    return await api<string[]>(`/api/pages/paths${suffix}`)
  }

  async function read(id: number) {
    return await api<PageDetail>(`/api/pages/${id}`)
  }

  async function create(input: PageInput) {
    return await api<PageSummary>('/api/pages', {
      method: 'POST',
      body: JSON.stringify(input),
    })
  }

  async function update(id: number, input: PageInput) {
    return await api<PageSummary>(`/api/pages/${id}`, {
      method: 'PUT',
      body: JSON.stringify(input),
    })
  }

  async function remove(id: number) {
    await apiVoid(`/api/pages/${id}`, { method: 'DELETE' })
    items.value = items.value.filter((p) => p.id !== id)
  }

  async function readRevision(pageId: number, revId: number) {
    return await api<RevisionDetail>(`/api/pages/${pageId}/revisions/${revId}`)
  }

  async function restoreRevision(pageId: number, revId: number) {
    return await api<PageSummary>(`/api/pages/${pageId}/revisions/${revId}/restore`, {
      method: 'POST',
    })
  }

  async function exportPage(id: number, format: 'pdf' | 'slides') {
    return await apiBlob(`/api/export/pages/${id}?format=${format}`)
  }

  return {
    items,
    load,
    loadPaths,
    read,
    readRevision,
    create,
    update,
    remove,
    restoreRevision,
    exportPage,
  }
})
