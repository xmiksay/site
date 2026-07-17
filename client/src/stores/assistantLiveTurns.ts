import { ref, type Ref } from 'vue'
import { useWsStore } from './ws'
import type { AssistantSessionDetail, LiveSubAgentTurn, LiveToolCall, LiveTurn } from '../types'

// Real, token-level streaming from the entanglement engine (issue #16
// connecting to #15's `Holly`) — `src/ai/ws_bridge.rs` forwards the
// engine's `OutEvent`s more or less verbatim, tagged with `db_session_id`
// (the engine only knows its own `SessionId`, not this DB row's id).
// `text_delta`/`reasoning_delta`/`tool_call*` accumulate into `live` for
// in-progress rendering; on settle (`done`/`error`/`session_hibernated`)
// the authoritative message list comes from a REST refetch rather than
// hand-folding the event stream client-side a second time — the fold
// logic (`ai::projection::project`) already exists once, in Rust.
//
// Split out of `stores/assistant.ts` (which calls this once from its setup
// body) to keep that file under the project's line cap — folding the WS
// event stream is a distinct concern from session/CRUD management.
export function useLiveTurns(
  current: Ref<AssistantSessionDetail | null>,
  sending: Ref<boolean>,
  loadSession: (id: number) => Promise<AssistantSessionDetail>,
) {
  // The turn currently streaming, if any — see `LiveTurn`'s doc. `null` when
  // no session has an in-flight turn right now.
  const live = ref<LiveTurn | null>(null)
  // Live turns for sub-agents (`researcher`/`page-writer`) spawned mid-turn,
  // keyed by their own `agentSessionId` — NOT nested inside `live`, since a
  // child keeps streaming (and can reach its own `done`) well after the
  // root's own `live` turn has already cleared. See `LiveSubAgentTurn`'s doc.
  const liveSubAgents = ref<Record<string, LiveSubAgentTurn>>({})

  function ensureLive(sessionId: number): LiveTurn {
    if (!live.value || live.value.sessionId !== sessionId) {
      live.value = { sessionId, text: '', reasoning: '', toolCalls: [] }
    }
    return live.value
  }

  function ensureLiveToolCall(sessionId: number, id: string, name: string): LiveToolCall {
    const turn = ensureLive(sessionId)
    let call = turn.toolCalls.find((c) => c.id === id)
    if (!call) {
      call = { id, name, argsText: '', args: undefined, status: 'pending' }
      turn.toolCalls.push(call)
    }
    return call
  }

  function ensureLiveSubAgent(
    dbSessionId: number,
    agentSessionId: string,
    profile?: string,
  ): LiveSubAgentTurn {
    let turn = liveSubAgents.value[agentSessionId]
    if (!turn) {
      turn = {
        agentSessionId,
        dbSessionId,
        profile: profile ?? '',
        text: '',
        reasoning: '',
        toolCalls: [],
        done: false,
      }
      liveSubAgents.value[agentSessionId] = turn
    } else if (profile && !turn.profile) {
      turn.profile = profile
    }
    return turn
  }

  function ensureLiveSubAgentToolCall(
    dbSessionId: number,
    agentSessionId: string,
    id: string,
    name: string,
  ): LiveToolCall {
    const turn = ensureLiveSubAgent(dbSessionId, agentSessionId)
    let call = turn.toolCalls.find((c) => c.id === id)
    if (!call) {
      call = { id, name, argsText: '', args: undefined, status: 'pending' }
      turn.toolCalls.push(call)
    }
    return call
  }

  // A sub-agent's events carry the same envelope kinds as the root's own
  // turn, just tagged with `agent_session_id` instead of belonging to the
  // root — route them into `liveSubAgents` instead of `ensureLive`/`live`.
  function handleSubAgentEvent(
    event: string,
    dbSessionId: number,
    agentSessionId: string,
    payload: Record<string, any>,
  ) {
    switch (event) {
      case 'session_started':
        ensureLiveSubAgent(dbSessionId, agentSessionId, payload.profile)
        break
      case 'text_delta':
        ensureLiveSubAgent(dbSessionId, agentSessionId).text += payload.text ?? ''
        break
      case 'reasoning_delta':
        ensureLiveSubAgent(dbSessionId, agentSessionId).reasoning += payload.text ?? ''
        break
      case 'tool_call_delta': {
        const call = ensureLiveSubAgentToolCall(
          dbSessionId,
          agentSessionId,
          payload.request_id,
          payload.tool,
        )
        call.argsText += payload.delta ?? ''
        break
      }
      case 'tool_call':
      case 'tool_request': {
        const call = ensureLiveSubAgentToolCall(
          dbSessionId,
          agentSessionId,
          payload.request_id,
          payload.tool,
        )
        call.argsText = payload.input ?? call.argsText
        try {
          call.args = JSON.parse(payload.input)
        } catch {
          call.args = payload.input
        }
        call.status = event === 'tool_request' ? 'requires_approval' : 'pending'
        break
      }
      case 'tool_output': {
        const call = liveSubAgents.value[agentSessionId]?.toolCalls.find(
          (c) => c.id === payload.request_id,
        )
        if (call) {
          call.status = 'done'
          call.output = payload.output
        }
        break
      }
      case 'done':
      case 'error':
      case 'session_hibernated':
        // Unlike the root's own turn, a finished child is simply dropped —
        // its transcript lives in `sub_agents` on the REST refetch below,
        // there's no separate "settled but still displayed" state for it.
        delete liveSubAgents.value[agentSessionId]
        if (current.value?.id === dbSessionId) {
          loadSession(dbSessionId).catch(() => {
            // best-effort — see the matching comment on the root-turn branch
          })
        }
        break
    }
  }

  useWsStore().on('assistant', (envelope) => {
    const payload = envelope.payload as Record<string, any>
    const sessionId = payload.db_session_id as number | undefined
    if (typeof sessionId !== 'number') return

    const agentSessionId = payload.agent_session_id as string | undefined
    if (agentSessionId) {
      handleSubAgentEvent(envelope.event, sessionId, agentSessionId, payload)
      return
    }

    switch (envelope.event) {
      case 'status':
        if (
          (payload.state === 'thinking' || payload.state === 'waiting_approval') &&
          current.value?.id === sessionId
        ) {
          sending.value = true
        }
        break
      case 'text_delta':
        ensureLive(sessionId).text += payload.text ?? ''
        break
      case 'reasoning_delta':
        ensureLive(sessionId).reasoning += payload.text ?? ''
        break
      case 'tool_call_delta': {
        const call = ensureLiveToolCall(sessionId, payload.request_id, payload.tool)
        call.argsText += payload.delta ?? ''
        break
      }
      case 'tool_call':
      case 'tool_request': {
        const call = ensureLiveToolCall(sessionId, payload.request_id, payload.tool)
        call.argsText = payload.input ?? call.argsText
        try {
          call.args = JSON.parse(payload.input)
        } catch {
          call.args = payload.input
        }
        call.status = envelope.event === 'tool_request' ? 'requires_approval' : 'pending'
        break
      }
      case 'tool_output': {
        const call = live.value?.toolCalls.find((c) => c.id === payload.request_id)
        if (call) {
          call.status = 'done'
          call.output = payload.output
        }
        break
      }
      case 'done':
      case 'error':
      case 'session_hibernated':
        if (live.value?.sessionId === sessionId) live.value = null
        if (current.value?.id === sessionId) {
          sending.value = false
          loadSession(sessionId).catch(() => {
            // best-effort — the initiating tab already has the authoritative
            // detail from its own REST response; other tabs retry on next
            // manual reselect if this transient refetch fails
          })
        }
        break
    }
  })

  return { live, liveSubAgents }
}
