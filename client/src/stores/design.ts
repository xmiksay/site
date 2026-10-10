import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api, apiBlob } from '../api'
import { designFileUrl } from '../lib/designPaths'
import type { DesignSource } from '../lib/designPaths'
import { publishRejection } from '../lib/designPublish'
import type { PublishOutcome } from '../lib/designPublish'
import { useWsStore } from './ws'
import type { DesignHistoryEntry, DesignState } from '../types'

export const useDesignStore = defineStore('design', () => {
  const state = ref<DesignState | null>(null)
  const files = computed(() => state.value?.files ?? [])
  const history = ref<DesignHistoryEntry[]>([])
  // Bumped on every draft/publish change, ours or another surface's (the AI,
  // an external agent, another tab): the preview and the open editor refresh
  // on it. Our own writes bump it too, so it works without the WS hub; the
  // WS echo of the same change lands a moment later and consumers debounce.
  const revision = ref(0)

  function adopt(next: DesignState) {
    state.value = next
    revision.value++
  }

  async function load() {
    state.value = await api<DesignState>('/api/design/draft')
  }

  async function reload() {
    try {
      adopt(await api<DesignState>('/api/design/reload', { method: 'POST' }))
    } catch (e) {
      // A failed reload is also recorded in `last_reload`; refresh so the header shows it.
      await load().catch(() => {})
      throw e
    }
  }

  // On failure (e.g. 422 template compile error) the server changed nothing, so
  // `state` is left untouched and the error propagates to the caller's editor.
  async function save(path: string, body: Blob | string) {
    adopt(
      await api<DesignState>(designFileUrl(path), {
        method: 'PUT',
        body,
        headers: { 'Content-Type': 'application/octet-stream' },
      }),
    )
  }

  async function remove(path: string) {
    adopt(await api<DesignState>(designFileUrl(path), { method: 'DELETE' }))
  }

  /** Resolves with the outcome for a 409/422 (nothing changed on the server);
   *  rejects only on other failures. */
  async function publish(force = false): Promise<PublishOutcome> {
    let entry: DesignHistoryEntry
    try {
      entry = await api<DesignHistoryEntry>(`/api/design/publish${force ? '?force=true' : ''}`, {
        method: 'POST',
      })
    } catch (e) {
      return publishRejection(e)
    }
    await load()
    revision.value++
    history.value = [entry, ...history.value.filter((h) => h.id !== entry.id)]
    return { kind: 'published', entry }
  }

  async function discard() {
    adopt(await api<DesignState>('/api/design/draft/discard', { method: 'POST' }))
  }

  async function loadHistory() {
    history.value = await api<DesignHistoryEntry[]>('/api/design/history')
  }

  async function restore(id: string) {
    adopt(
      await api<DesignState>(`/api/design/history/${encodeURIComponent(id)}/restore`, {
        method: 'POST',
      }),
    )
  }

  /** Draft preview for this browser: the public site renders the draft. */
  async function setPreview(on: boolean) {
    await api<{ on: boolean }>('/api/design/preview', {
      method: 'POST',
      body: JSON.stringify({ on }),
    })
  }

  async function fetchContent(path: string, source: DesignSource = 'draft'): Promise<Blob> {
    return (await apiBlob(designFileUrl(path, source))).blob
  }

  async function fetchText(path: string, source: DesignSource = 'draft'): Promise<string> {
    return await (await fetchContent(path, source)).text()
  }

  useWsStore().on('design', (envelope) => {
    // Nothing to refresh until a page has loaded the draft.
    if (!state.value) {
      revision.value++
      return
    }
    // Bump only once the fresh state landed: consumers read `files` (e.g.
    // a file's `overridden`) when the revision moves.
    load()
      .catch(() => {})
      .finally(() => revision.value++)
    if (envelope.event === 'published' && history.value.length) loadHistory().catch(() => {})
  })

  return {
    state,
    files,
    history,
    revision,
    load,
    reload,
    save,
    remove,
    publish,
    discard,
    loadHistory,
    restore,
    setPreview,
    fetchContent,
    fetchText,
  }
})
