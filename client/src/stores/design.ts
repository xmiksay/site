import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api, apiBlob } from '../api'
import { designFileUrl } from '../lib/designPaths'
import type { DesignHistoryEntry, DesignState } from '../types'

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

  // A 409 means design/ changed outside the draft (retry with `force`) or
  // there is nothing to publish; the caller shows the server's message.
  async function publish(force = false): Promise<DesignHistoryEntry> {
    const entry = await api<DesignHistoryEntry>(
      `/api/design/publish${force ? '?force=true' : ''}`,
      { method: 'POST' },
    )
    await load()
    return entry
  }

  async function discard() {
    state.value = await api<DesignState>('/api/design/draft/discard', { method: 'POST' })
  }

  async function fetchContent(path: string, baked = false): Promise<Blob> {
    return (await apiBlob(designFileUrl(path, baked))).blob
  }

  async function fetchText(path: string, baked = false): Promise<string> {
    return await (await fetchContent(path, baked)).text()
  }

  return { state, files, load, reload, save, remove, publish, discard, fetchContent, fetchText }
})
