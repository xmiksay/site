import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
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
        tool: 'edit_page',
        input: '{"path":"a"}',
      }),
    )

    expect(store.liveSubAgents['child-1'].text).toBe('hi there')
    expect(store.liveSubAgents['child-1'].toolCalls).toHaveLength(1)
    expect(store.liveSubAgents['child-1'].toolCalls[0]).toMatchObject({
      id: 'call-1',
      name: 'edit_page',
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
