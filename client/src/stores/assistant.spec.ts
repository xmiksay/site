import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { nextTick } from 'vue'
import { useAssistantStore } from './assistant'
import { api } from '../api'
import type { WsEnvelope } from '../types'

vi.mock('../api', async (importActual) => {
  const actual = await importActual<typeof import('../api')>()
  return { ...actual, api: vi.fn(), apiVoid: vi.fn() }
})

// The store registers its WS handler via `useWsStore().on('assistant', ...)`
// at store-creation time — stub that store to capture the handler so tests
// can feed it synthetic envelopes directly, the same shape `ws_bridge.rs`
// sends over the wire.
let wsHandler: ((envelope: WsEnvelope) => void) | undefined
vi.mock('./ws', () => ({
  useWsStore: () => ({
    on: (_topic: string, handler: (envelope: WsEnvelope) => void) => {
      wsHandler = handler
      return () => {}
    },
  }),
}))

const apiMock = vi.mocked(api)

function envelope(event: string, payload: Record<string, any>): WsEnvelope {
  return { topic: 'assistant', event, payload }
}

describe('assistant store — sub-agent live routing', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    wsHandler = undefined
  })

  it('session_started creates a liveSubAgents entry keyed by agent_session_id', () => {
    const store = useAssistantStore()
    expect(wsHandler).toBeDefined()

    wsHandler!(
      envelope('session_started', {
        session: 'child-1',
        parent: 'root-1',
        profile: 'researcher',
        db_session_id: 42,
        agent_session_id: 'child-1',
      }),
    )

    expect(store.liveSubAgents['child-1']).toMatchObject({
      agentSessionId: 'child-1',
      dbSessionId: 42,
      profile: 'researcher',
      text: '',
    })
  })

  it('accumulates text_delta/tool_call events into the child entry, not the root live turn', () => {
    const store = useAssistantStore()

    wsHandler!(
      envelope('session_started', {
        profile: 'page-writer',
        db_session_id: 42,
        agent_session_id: 'child-1',
      }),
    )
    wsHandler!(
      envelope('text_delta', { db_session_id: 42, agent_session_id: 'child-1', text: 'hi ' }),
    )
    wsHandler!(
      envelope('text_delta', { db_session_id: 42, agent_session_id: 'child-1', text: 'there' }),
    )
    wsHandler!(
      envelope('tool_call', {
        db_session_id: 42,
        agent_session_id: 'child-1',
        request_id: 'call-1',
        tool: 'page_edit',
        input: '{"path":"a"}',
      }),
    )

    expect(store.liveSubAgents['child-1'].text).toBe('hi there')
    expect(store.liveSubAgents['child-1'].toolCalls).toHaveLength(1)
    expect(store.liveSubAgents['child-1'].toolCalls[0]).toMatchObject({
      id: 'call-1',
      name: 'page_edit',
      args: { path: 'a' },
    })
    expect(store.live).toBeNull()
  })

  it('clears the child entry on its own done, independent of the root live turn', async () => {
    const store = useAssistantStore()

    wsHandler!(
      envelope('session_started', {
        profile: 'researcher',
        db_session_id: 42,
        agent_session_id: 'child-1',
      }),
    )
    expect(store.liveSubAgents['child-1']).toBeDefined()

    wsHandler!(envelope('done', { db_session_id: 42, agent_session_id: 'child-1' }))

    expect(store.liveSubAgents['child-1']).toBeUndefined()
    // `current` was never set to session 42 in this test, so no refetch fires.
    expect(apiMock).not.toHaveBeenCalled()
  })

  it('root-only events (no agent_session_id) still go through the existing live-turn path', () => {
    const store = useAssistantStore()

    wsHandler!(envelope('text_delta', { db_session_id: 42, text: 'root text' }))

    expect(store.live).toMatchObject({ sessionId: 42, text: 'root text' })
    expect(Object.keys(store.liveSubAgents)).toHaveLength(0)
  })
})

// The session list is the tree's only input (#103), so the summary must carry
// the parentage columns m_032 added (#99) through the store untouched.
describe('assistant store — session summaries', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('keeps parent_session_id / root_engine_session_id on the listed sessions', async () => {
    const store = useAssistantStore()
    apiMock.mockResolvedValueOnce([
      { id: 1, title: 'root', parent_session_id: null, root_engine_session_id: 'u1:root' },
      {
        id: 2,
        title: 'researcher',
        parent_session_id: 1,
        root_engine_session_id: 'u1:root',
        agent_profile: 'researcher',
      },
    ] as any)

    await store.loadSessions()

    expect(store.sessions[0]).toMatchObject({ parent_session_id: null })
    expect(store.sessions[1]).toMatchObject({
      parent_session_id: 1,
      root_engine_session_id: 'u1:root',
      agent_profile: 'researcher',
    })
  })

  it('refetches the list after a delete, since the root cascades onto its children', async () => {
    const store = useAssistantStore()
    apiMock.mockResolvedValueOnce([
      { id: 1, parent_session_id: null },
      { id: 2, parent_session_id: 1 },
    ] as any)
    await store.loadSessions()

    apiMock.mockResolvedValueOnce([] as any)
    await store.deleteSession(1)

    // Not just `[{id: 2}]` — the child row is gone server-side too.
    expect(store.sessions).toEqual([])
  })

  it('a turn finishing for another session neither takes over nor blocks the open chat', async () => {
    const store = useAssistantStore()
    store.current = { id: 1, messages: [] } as any
    let finish!: (d: unknown) => void
    apiMock.mockReturnValueOnce(new Promise((r) => (finish = r)) as never)
    const turn = store.sendMessage(1, 'hi')
    expect(store.sending).toBe(true)

    // The admin switches to the Design studio's chat while the turn runs.
    store.current = { id: 2, messages: [] } as any
    await nextTick()
    expect(store.sending).toBe(false)

    finish({ id: 1, messages: [{ id: 9 }] })
    expect(await turn).toEqual({ id: 1, messages: [{ id: 9 }] })
    expect(store.current?.id).toBe(2)
    expect(store.sending).toBe(false)

    // Back on the first chat, nothing is in flight any more.
    store.current = { id: 1, messages: [] } as any
    await nextTick()
    expect(store.sending).toBe(false)
  })

  it('switching back to a chat with a request in flight shows it busy', async () => {
    const store = useAssistantStore()
    store.current = { id: 1, messages: [] } as any
    let finish!: (d: unknown) => void
    apiMock.mockReturnValueOnce(new Promise((r) => (finish = r)) as never)
    const turn = store.sendMessage(1, 'hi')
    store.current = { id: 2, messages: [] } as any
    await nextTick()
    store.current = { id: 1, messages: [] } as any
    await nextTick()
    expect(store.sending).toBe(true)
    finish({ id: 1, messages: [] })
    await turn
    expect(store.sending).toBe(false)
    expect(store.current?.id).toBe(1)
  })
})
