import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useWsStore } from './ws'

// The real WebSocket in jsdom actually tries to open a socket, so stub a
// minimal fake that lets tests drive `open`/`message`/`close`/`error`
// listeners directly and inspect what the store did with them.
class FakeWebSocket {
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3
  static instances: FakeWebSocket[] = []

  readyState = FakeWebSocket.CONNECTING
  url: string
  private listeners: Record<string, Array<(ev?: any) => void>> = {}

  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }

  addEventListener(type: string, handler: (ev?: any) => void) {
    ;(this.listeners[type] ??= []).push(handler)
  }

  dispatch(type: string, ev?: any) {
    for (const h of this.listeners[type] ?? []) h(ev)
  }

  close() {
    this.readyState = FakeWebSocket.CLOSED
    this.dispatch('close')
  }
}

describe('ws store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    FakeWebSocket.instances = []
    vi.stubGlobal('WebSocket', FakeWebSocket)
  })

  afterEach(() => {
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  it('connect opens a socket to /api/ws on the current host', () => {
    const store = useWsStore()
    store.connect()
    expect(FakeWebSocket.instances).toHaveLength(1)
    expect(FakeWebSocket.instances[0].url).toBe(`ws://${location.host}/api/ws`)
  })

  it('connect is a no-op while a socket is already open or connecting', () => {
    const store = useWsStore()
    store.connect()
    store.connect()
    expect(FakeWebSocket.instances).toHaveLength(1)
  })

  it('sets connected true on open and false again on close', () => {
    const store = useWsStore()
    store.connect()
    const socket = FakeWebSocket.instances[0]

    socket.readyState = FakeWebSocket.OPEN
    socket.dispatch('open')
    expect(store.connected).toBe(true)

    socket.close()
    expect(store.connected).toBe(false)
  })

  it('routes a parsed message to handlers registered for its topic only', () => {
    const store = useWsStore()
    store.connect()
    const socket = FakeWebSocket.instances[0]

    const pagesHandler = vi.fn()
    const filesHandler = vi.fn()
    store.on('pages', pagesHandler)
    store.on('files', filesHandler)

    socket.dispatch('message', {
      data: JSON.stringify({ topic: 'pages', event: 'updated', payload: { id: 1 } }),
    })

    expect(pagesHandler).toHaveBeenCalledWith({
      topic: 'pages',
      event: 'updated',
      payload: { id: 1 },
    })
    expect(filesHandler).not.toHaveBeenCalled()
  })

  it('silently ignores malformed message payloads', () => {
    const store = useWsStore()
    store.connect()
    const socket = FakeWebSocket.instances[0]
    const handler = vi.fn()
    store.on('pages', handler)

    expect(() => socket.dispatch('message', { data: 'not json' })).not.toThrow()
    expect(handler).not.toHaveBeenCalled()
  })

  it('on() returns an unsubscribe function that stops further delivery', () => {
    const store = useWsStore()
    store.connect()
    const socket = FakeWebSocket.instances[0]
    const handler = vi.fn()
    const off = store.on('pages', handler)
    off()

    socket.dispatch('message', {
      data: JSON.stringify({ topic: 'pages', event: 'updated', payload: {} }),
    })
    expect(handler).not.toHaveBeenCalled()
  })

  it('error handling closes the socket', () => {
    const store = useWsStore()
    store.connect()
    const socket = FakeWebSocket.instances[0]
    const closeSpy = vi.spyOn(socket, 'close')

    socket.dispatch('error')
    expect(closeSpy).toHaveBeenCalled()
  })

  it('reconnects with exponential backoff after an unexpected close', () => {
    vi.useFakeTimers()
    const store = useWsStore()
    store.connect()
    FakeWebSocket.instances[0].close()
    expect(FakeWebSocket.instances).toHaveLength(1)

    vi.advanceTimersByTime(1000)
    expect(FakeWebSocket.instances).toHaveLength(2)

    FakeWebSocket.instances[1].close()
    vi.advanceTimersByTime(1999)
    expect(FakeWebSocket.instances).toHaveLength(2)
    vi.advanceTimersByTime(1)
    expect(FakeWebSocket.instances).toHaveLength(3)
  })

  it('disconnect disables reconnect and does not schedule another socket', () => {
    vi.useFakeTimers()
    const store = useWsStore()
    store.connect()
    store.disconnect()
    expect(store.connected).toBe(false)

    vi.advanceTimersByTime(60000)
    expect(FakeWebSocket.instances).toHaveLength(1)
  })
})
