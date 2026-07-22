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
      live.value = { sessionId, text: '', reasoning: '', toolCalls: [], retrying: false }
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

  // The one place a live tool call is ever marked settled — called both from
  // the `tool_output` WS handlers below (with the real output) and directly
  // from the Approve/Reject buttons the instant their POST resolves
  // (`stores/assistant.ts`'s `resolveLiveToolCall`), so the UI doesn't sit on
  // `requires_approval` waiting for a WS round-trip that may be delayed or
  // dropped (the stuck-approval bug). Calling it twice for the same id is
  // harmless: the click path calls it with no `output` (just clears the
  // buttons), and the `tool_output` event that eventually arrives calls it
  // again with the real output, filling that in — never a hard requirement,
  // never a double-processing error.
  function resolveLiveToolCall(callId: string, agentSessionId?: string, output?: string) {
    const calls = agentSessionId
      ? liveSubAgents.value[agentSessionId]?.toolCalls
      : live.value?.toolCalls
    const call = calls?.find((c) => c.id === callId)
    if (!call) return
    call.status = 'done'
    if (output !== undefined) call.output = output
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
      case 'tool_output':
        resolveLiveToolCall(payload.request_id, agentSessionId, payload.output)
        break
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
      case 'text_delta': {
        const turn = ensureLive(sessionId)
        turn.text += payload.text ?? ''
        turn.retrying = false
        break
      }
      case 'reasoning_delta':
        ensureLive(sessionId).reasoning += payload.text ?? ''
        break
      case 'ambiguous_retry':
        // #88 (ADR-0118): core committed whatever partial text streamed and
        // is silently re-requesting within the same turn — surface it as a
        // transient chip rather than leaving the user staring at dead air.
        // Cleared by the next `text_delta`/`done`/`error` above/below.
        ensureLive(sessionId).retrying = true
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
      case 'tool_output':
        resolveLiveToolCall(payload.request_id, undefined, payload.output)
        break
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
      case 'compacted':
        // #40: a `/compact` fork repoints this DB row's `engine_session_id`
        // to a fresh successor session — any *other* open tab on it (the
        // initiating tab already has the authoritative detail from its own
        // REST response) needs to drop its stale live turn and refetch, or
        // it keeps rendering/targeting a now-retired session.
        if (live.value?.sessionId === sessionId) live.value = null
        if (current.value?.id === sessionId) {
          sending.value = false
          loadSession(sessionId).catch(() => {
            // best-effort — see the matching comment on the settle branch above
          })
        }
        break
      case 'model_changed':
      case 'generation_changed':
      case 'agent_changed':
        // #42: confirmation-of-write events, not mid-turn lifecycle — a
        // session's model/generation/profile changed (from any tab). Just
        // refetch so an open tab picks up the new value; `live`/`sending`
        // are untouched since there's no turn in flight.
        if (current.value?.id === sessionId) {
          loadSession(sessionId).catch(() => {
            // best-effort — see the matching comment on the settle branch above
          })
        }
        break
    }
  })

  return { live, liveSubAgents, resolveLiveToolCall }
}
