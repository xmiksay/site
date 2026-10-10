import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api, apiBlob } from '../api'
import { designFileUrl } from '../lib/designPaths'
import type { DesignState } from '../types'

export const useDesignStore = defineStore('design', () => {
  const state = ref<DesignState | null>(null)
  const files = computed(() => state.value?.files ?? [])

  async function load() {
    state.value = await api<DesignState>('/api/design/draft')
  }

  async function reload() {
    try {
      state.value = await api<DesignState>('/api/design/reload', { method: 'POST' })
    } catch (e) {
      // A failed reload is also recorded in `last_reload`; refresh so the header shows it.
      await load().catch(() => {})
      throw e
    }
  }

  // On failure (e.g. 422 template compile error) the server changed nothing, so
  // `state` is left untouched and the error propagates to the caller's editor.
  async function save(path: string, body: Blob | string) {
    state.value = await api<DesignState>(designFileUrl(path), {
      method: 'PUT',
      body,
      headers: { 'Content-Type': 'application/octet-stream' },
    })
  }

  async function remove(path: string) {
    state.value = await api<DesignState>(designFileUrl(path), { method: 'DELETE' })
  }

  async function fetchContent(path: string, baked = false): Promise<Blob> {
    return (await apiBlob(designFileUrl(path, baked))).blob
  }

  async function fetchText(path: string, baked = false): Promise<string> {
    return await (await fetchContent(path, baked)).text()
  }

  return { state, files, load, reload, save, remove, fetchContent, fetchText }
})
