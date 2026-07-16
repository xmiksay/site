import { defineStore } from 'pinia'
import { ref } from 'vue'
import type { WsEnvelope, WsTopic } from '../types'

type Handler = (envelope: WsEnvelope) => void

const INITIAL_BACKOFF_MS = 1000
const MAX_BACKOFF_MS = 30000

export const useWsStore = defineStore('ws', () => {
  const connected = ref(false)

  let socket: WebSocket | null = null
  let reconnectEnabled = false
  let backoffMs = INITIAL_BACKOFF_MS
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null
  const handlers = new Map<WsTopic, Set<Handler>>()

  function connect() {
    if (socket && (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)) {
      return
    }
    reconnectEnabled = true
    open()
  }

  function open() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:'
    socket = new WebSocket(`${proto}//${location.host}/api/ws`)

    socket.addEventListener('open', () => {
      connected.value = true
      backoffMs = INITIAL_BACKOFF_MS
    })

    socket.addEventListener('message', (event) => {
      let envelope: WsEnvelope
      try {
        envelope = JSON.parse(event.data)
      } catch {
        return
      }
      const set = handlers.get(envelope.topic)
      if (!set) return
      for (const handler of set) handler(envelope)
    })

    socket.addEventListener('close', () => {
      connected.value = false
      socket = null
      if (!reconnectEnabled) return
      reconnectTimer = setTimeout(() => {
        backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS)
        open()
      }, backoffMs)
    })

    socket.addEventListener('error', () => {
      socket?.close()
    })
  }

  function disconnect() {
    reconnectEnabled = false
    if (reconnectTimer) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
    backoffMs = INITIAL_BACKOFF_MS
    socket?.close()
    socket = null
    connected.value = false
  }

  function on(topic: WsTopic, handler: Handler): () => void {
    let set = handlers.get(topic)
    if (!set) {
      set = new Set()
      handlers.set(topic, set)
    }
    set.add(handler)
    return () => {
      set?.delete(handler)
    }
  }

  return { connected, connect, disconnect, on }
})
