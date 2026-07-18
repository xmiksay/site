import { describe, it, expect, beforeEach, vi } from 'vitest'
import { ref, type Ref } from 'vue'
import { useLiveTurns } from './assistantLiveTurns'
import type { AssistantSessionDetail, WsEnvelope } from '../types'

// `useLiveTurns` registers its handler via `useWsStore().on('assistant', ...)`
// at call time — stub that store to capture the handler so tests can feed it
// synthetic envelopes directly, the same shape `ws_bridge.rs` sends over the
// wire (mirrors `assistant.spec.ts`'s approach for the same seam).
let wsHandler: ((envelope: WsEnvelope) => void) | undefined
vi.mock('./ws', () => ({
  useWsStore: () => ({
    on: (_topic: string, handler: (envelope: WsEnvelope) => void) => {
      wsHandler = handler
      return () => {}
    },
  }),
}))

function envelope(event: string, payload: Record<string, any>): WsEnvelope {
  return { topic: 'assistant', event, payload }
}

describe('useLiveTurns', () => {
  let current: Ref<AssistantSessionDetail | null>
  let sending: Ref<boolean>
  let loadSession: ReturnType<typeof vi.fn<(id: number) => Promise<AssistantSessionDetail>>>

  beforeEach(() => {
    vi.clearAllMocks()
    wsHandler = undefined
    current = ref<AssistantSessionDetail | null>(null)
    sending = ref(false)
    loadSession = vi.fn().mockResolvedValue(undefined)
  })

  function setup() {
    return useLiveTurns(current, sending, loadSession)
  }

  it('starts with no live turn and no sub-agents', () => {
    const { live, liveSubAgents } = setup()
    expect(live.value).toBeNull()
    expect(liveSubAgents.value).toEqual({})
  })

  it('ignores envelopes without a numeric db_session_id', () => {
    const { live } = setup()
    wsHandler!(envelope('text_delta', { text: 'nope' }))
    expect(live.value).toBeNull()
  })

  it('accumulates text_delta and reasoning_delta into the live turn for the session', () => {
    const { live } = setup()
    wsHandler!(envelope('text_delta', { db_session_id: 1, text: 'hel' }))
    wsHandler!(envelope('text_delta', { db_session_id: 1, text: 'lo' }))
    wsHandler!(envelope('reasoning_delta', { db_session_id: 1, text: 'thinking' }))
    expect(live.value).toMatchObject({ sessionId: 1, text: 'hello', reasoning: 'thinking' })
  })

  it('starting deltas for a new session replaces the live turn for the old one', () => {
    const { live } = setup()
    wsHandler!(envelope('text_delta', { db_session_id: 1, text: 'a' }))
    wsHandler!(envelope('text_delta', { db_session_id: 2, text: 'b' }))
    expect(live.value).toMatchObject({ sessionId: 2, text: 'b' })
  })

  it('tool_call_delta accumulates argsText, tool_call parses JSON args and marks pending', () => {
    const { live } = setup()
    wsHandler!(
      envelope('tool_call_delta', {
        db_session_id: 1,
        request_id: 'c1',
        tool: 'edit_page',
        delta: '{"path"',
      }),
    )
    wsHandler!(
      envelope('tool_call_delta', {
        db_session_id: 1,
        request_id: 'c1',
        tool: 'edit_page',
        delta: ':"a"}',
      }),
    )
    wsHandler!(
      envelope('tool_call', {
        db_session_id: 1,
        request_id: 'c1',
        tool: 'edit_page',
        input: '{"path":"a"}',
      }),
    )

    const call = live.value!.toolCalls[0]
    expect(call).toMatchObject({
      id: 'c1',
      name: 'edit_page',
      argsText: '{"path":"a"}',
      args: { path: 'a' },
      status: 'pending',
    })
  })

  it('tool_request marks the call requires_approval instead of pending', () => {
    const { live } = setup()
    wsHandler!(
      envelope('tool_request', {
        db_session_id: 1,
        request_id: 'c1',
        tool: 'delete_page',
        input: '{"id":1}',
      }),
    )
    expect(live.value!.toolCalls[0].status).toBe('requires_approval')
  })

  it('falls back to storing the raw input string when JSON.parse fails', () => {
    const { live } = setup()
    wsHandler!(
      envelope('tool_call', {
        db_session_id: 1,
        request_id: 'c1',
        tool: 'edit_page',
        input: 'not json',
      }),
    )
    expect(live.value!.toolCalls[0].args).toBe('not json')
  })

  it('tool_output marks the matching call done and stores output', () => {
    const { live } = setup()
    wsHandler!(
      envelope('tool_call', { db_session_id: 1, request_id: 'c1', tool: 'edit_page', input: '{}' }),
    )
    wsHandler!(envelope('tool_output', { db_session_id: 1, request_id: 'c1', output: 'ok' }))
    expect(live.value!.toolCalls[0]).toMatchObject({ status: 'done', output: 'ok' })
  })

  it('status thinking sets sending when it is the current session', () => {
    current.value = { id: 1 } as AssistantSessionDetail
    setup()
    wsHandler!(envelope('status', { db_session_id: 1, state: 'thinking' }))
    expect(sending.value).toBe(true)
  })

  it('status thinking for a different session leaves sending untouched', () => {
    current.value = { id: 2 } as AssistantSessionDetail
    setup()
    wsHandler!(envelope('status', { db_session_id: 1, state: 'thinking' }))
    expect(sending.value).toBe(false)
  })

  it('done clears the live turn and refetches when it is the current session', async () => {
    current.value = { id: 1 } as AssistantSessionDetail
    sending.value = true
    const { live } = setup()
    wsHandler!(envelope('text_delta', { db_session_id: 1, text: 'hi' }))
    wsHandler!(envelope('done', { db_session_id: 1 }))

    expect(live.value).toBeNull()
    expect(sending.value).toBe(false)
    expect(loadSession).toHaveBeenCalledWith(1)
  })

  it('done for a session that is not current does not refetch', () => {
    current.value = { id: 2 } as AssistantSessionDetail
    const { live } = setup()
    wsHandler!(envelope('text_delta', { db_session_id: 1, text: 'hi' }))
    wsHandler!(envelope('done', { db_session_id: 1 }))

    expect(live.value).toBeNull()
    expect(loadSession).not.toHaveBeenCalled()
  })

  it('error and session_hibernated settle the live turn the same way as done', () => {
    const { live } = setup()
    wsHandler!(envelope('text_delta', { db_session_id: 1, text: 'hi' }))
    wsHandler!(envelope('error', { db_session_id: 1 }))
    expect(live.value).toBeNull()

    wsHandler!(envelope('text_delta', { db_session_id: 1, text: 'hi again' }))
    wsHandler!(envelope('session_hibernated', { db_session_id: 1 }))
    expect(live.value).toBeNull()
  })

  it('a settle event does not clear a live turn belonging to a different session', () => {
    const { live } = setup()
    wsHandler!(envelope('text_delta', { db_session_id: 1, text: 'hi' }))
    wsHandler!(envelope('done', { db_session_id: 2 }))
    expect(live.value).toMatchObject({ sessionId: 1, text: 'hi' })
  })

  // ---- #42: model/generation/agent-profile change events ----

  it.each(['model_changed', 'generation_changed', 'agent_changed'])(
    '%s refetches when it is the current session, without touching live/sending',
    async (event) => {
      current.value = { id: 1 } as AssistantSessionDetail
      sending.value = true
      const { live } = setup()
      wsHandler!(envelope('text_delta', { db_session_id: 1, text: 'hi' }))
      wsHandler!(envelope(event, { db_session_id: 1 }))

      // Unlike `done`/`compacted`, these aren't turn-lifecycle events — the
      // in-progress turn and sending flag are left exactly as they were.
      expect(live.value).toMatchObject({ sessionId: 1, text: 'hi' })
      expect(sending.value).toBe(true)
      expect(loadSession).toHaveBeenCalledWith(1)
    },
  )

  it.each(['model_changed', 'generation_changed', 'agent_changed'])(
    '%s does not refetch when it is not the current session',
    (event) => {
      current.value = { id: 2 } as AssistantSessionDetail
      setup()
      wsHandler!(envelope(event, { db_session_id: 1 }))
      expect(loadSession).not.toHaveBeenCalled()
    },
  )

  // ---- sub-agent routing ----

  it('session_started with agent_session_id creates a liveSubAgents entry, not the root live turn', () => {
    const { live, liveSubAgents } = setup()
    wsHandler!(
      envelope('session_started', {
        db_session_id: 1,
        agent_session_id: 'child-1',
        profile: 'researcher',
      }),
    )
    expect(live.value).toBeNull()
    expect(liveSubAgents.value['child-1']).toMatchObject({
      agentSessionId: 'child-1',
      dbSessionId: 1,
      profile: 'researcher',
      text: '',
    })
  })

  it('accumulates sub-agent text_delta/tool_call independently of the root turn', () => {
    const { live, liveSubAgents } = setup()
    wsHandler!(envelope('text_delta', { db_session_id: 1, text: 'root' }))
    wsHandler!(
      envelope('text_delta', { db_session_id: 1, agent_session_id: 'child-1', text: 'child' }),
    )
    wsHandler!(
      envelope('tool_call', {
        db_session_id: 1,
        agent_session_id: 'child-1',
        request_id: 'c1',
        tool: 'search_pages',
        input: '{"q":"x"}',
      }),
    )

    expect(live.value).toMatchObject({ sessionId: 1, text: 'root' })
    expect(liveSubAgents.value['child-1'].text).toBe('child')
    expect(liveSubAgents.value['child-1'].toolCalls[0]).toMatchObject({
      id: 'c1',
      name: 'search_pages',
      args: { q: 'x' },
    })
  })

  it('sub-agent tool_output marks the matching child tool call done', () => {
    const { liveSubAgents } = setup()
    wsHandler!(
      envelope('tool_call', {
        db_session_id: 1,
        agent_session_id: 'child-1',
        request_id: 'c1',
        tool: 'search_pages',
        input: '{}',
      }),
    )
    wsHandler!(
      envelope('tool_output', {
        db_session_id: 1,
        agent_session_id: 'child-1',
        request_id: 'c1',
        output: 'found',
      }),
    )
    expect(liveSubAgents.value['child-1'].toolCalls[0]).toMatchObject({
      status: 'done',
      output: 'found',
    })
  })

  it('sub-agent done drops the entry and refetches when it belongs to the current session', () => {
    current.value = { id: 1 } as AssistantSessionDetail
    const { liveSubAgents } = setup()
    wsHandler!(
      envelope('session_started', { db_session_id: 1, agent_session_id: 'child-1' }),
    )
    wsHandler!(envelope('done', { db_session_id: 1, agent_session_id: 'child-1' }))

    expect(liveSubAgents.value['child-1']).toBeUndefined()
    expect(loadSession).toHaveBeenCalledWith(1)
  })

  it('sub-agent done for a non-current session drops the entry without refetching', () => {
    current.value = { id: 2 } as AssistantSessionDetail
    const { liveSubAgents } = setup()
    wsHandler!(
      envelope('session_started', { db_session_id: 1, agent_session_id: 'child-1' }),
    )
    wsHandler!(envelope('done', { db_session_id: 1, agent_session_id: 'child-1' }))

    expect(liveSubAgents.value['child-1']).toBeUndefined()
    expect(loadSession).not.toHaveBeenCalled()
  })
})
