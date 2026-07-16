import type { Ref } from 'vue'
import { useWsStore } from '../stores/ws'
import type { WsTopic } from '../types'

export function useListSync<T extends { id: number }>(topic: WsTopic, items: Ref<T[]>) {
  const ws = useWsStore()
  ws.on(topic, (envelope) => {
    const payload = envelope.payload as Partial<T> & { id?: number } | null | undefined
    if (envelope.event === 'deleted') {
      items.value = items.value.filter((i) => i.id !== payload?.id)
      return
    }
    if (!payload || typeof payload.id !== 'number') return
    const idx = items.value.findIndex((i) => i.id === payload.id)
    if (idx >= 0) items.value[idx] = payload as T
    else items.value.unshift(payload as T)
  })
}
