import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, apiVoid } from '../api'
import { useWsStore } from './ws'
import type {
  AssistantSession,
  AssistantSessionDetail,
  LiveToolCall,
  LiveTurn,
  LlmModel,
  LlmModelInput,
  LlmProvider,
  LlmProviderInput,
  McpServer,
  McpServerListResponse,
  ToolPermission,
  ToolPermissionInput,
} from '../types'

export const useAssistantStore = defineStore('assistant', () => {
  const sessions = ref<AssistantSession[]>([])
  const current = ref<AssistantSessionDetail | null>(null)
  const sending = ref(false)
  // The turn currently streaming, if any — see `LiveTurn`'s doc. `null` when
  // no session has an in-flight turn right now.
  const live = ref<LiveTurn | null>(null)

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

  // Real, token-level streaming from the entanglement engine (issue #16
  // connecting to #15's `Holly`) — `src/ai/ws_bridge.rs` forwards the
  // engine's `OutEvent`s more or less verbatim, tagged with `db_session_id`
  // (the engine only knows its own `SessionId`, not this DB row's id).
  // `text_delta`/`reasoning_delta`/`tool_call*` accumulate into `live` for
  // in-progress rendering; on settle (`done`/`error`/`session_hibernated`)
  // the authoritative message list comes from a REST refetch rather than
  // hand-folding the event stream client-side a second time — the fold
  // logic (`ai::projection::project`) already exists once, in Rust.
  useWsStore().on('assistant', (envelope) => {
    const payload = envelope.payload as Record<string, any>
    const sessionId = payload.db_session_id as number | undefined
    if (typeof sessionId !== 'number') return

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

  async function loadSessions() {
    sessions.value = await api<AssistantSession[]>('/api/assistant/sessions')
  }

  async function createSession(input: {
    title?: string
    model_id?: number
    enabled_mcp_server_ids?: number[]
  } = {}) {
    const created = await api<AssistantSession>('/api/assistant/sessions', {
      method: 'POST',
      body: JSON.stringify(input),
    })
    sessions.value.unshift(created)
    return created
  }

  async function loadSession(id: number) {
    current.value = await api<AssistantSessionDetail>(`/api/assistant/sessions/${id}`)
    return current.value
  }

  async function updateSession(
    id: number,
    input: { title?: string; model_id?: number; enabled_mcp_server_ids?: number[] },
  ) {
    const updated = await api<AssistantSession>(`/api/assistant/sessions/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(input),
    })
    const idx = sessions.value.findIndex((s) => s.id === id)
    if (idx >= 0) sessions.value[idx] = updated
    return updated
  }

  async function deleteSession(id: number) {
    await apiVoid(`/api/assistant/sessions/${id}`, { method: 'DELETE' })
    sessions.value = sessions.value.filter((s) => s.id !== id)
    if (current.value?.id === id) current.value = null
  }

  async function sendMessage(id: number, text: string) {
    sending.value = true
    try {
      current.value = await api<AssistantSessionDetail>(
        `/api/assistant/sessions/${id}/messages`,
        {
          method: 'POST',
          body: JSON.stringify({ text }),
        },
      )
    } finally {
      sending.value = false
    }
    return current.value
  }

  async function approveToolCalls(
    sessionId: number,
    messageId: number,
    decisions: Array<{ tool_call_id: string; approve: boolean; remember?: boolean }>,
  ) {
    sending.value = true
    try {
      current.value = await api<AssistantSessionDetail>(
        `/api/assistant/sessions/${sessionId}/messages/${messageId}/approve`,
        { method: 'POST', body: JSON.stringify({ decisions }) },
      )
    } finally {
      sending.value = false
    }
    return current.value
  }

  // ---- MCP servers ----
  const mcpServers = ref<McpServer[]>([])
  const discovered = ref<McpServerListResponse['discovered']>([])

  async function loadMcpServers() {
    const resp = await api<McpServerListResponse>('/api/assistant/mcp-servers')
    mcpServers.value = resp.user_servers
    discovered.value = resp.discovered
  }

  async function createMcpServer(input: {
    name: string
    url: string
    enabled?: boolean
    forward_user_token?: boolean
    headers?: Record<string, string>
  }) {
    const created = await api<McpServer>('/api/assistant/mcp-servers', {
      method: 'POST',
      body: JSON.stringify(input),
    })
    mcpServers.value.push(created)
    return created
  }

  async function updateMcpServer(
    id: number,
    input: {
      name?: string
      enabled?: boolean
      forward_user_token?: boolean
      url?: string
      headers?: Record<string, string>
    },
  ) {
    const updated = await api<McpServer>(`/api/assistant/mcp-servers/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(input),
    })
    const idx = mcpServers.value.findIndex((s) => s.id === id)
    if (idx >= 0) mcpServers.value[idx] = updated
    return updated
  }

  async function deleteMcpServer(id: number) {
    await apiVoid(`/api/assistant/mcp-servers/${id}`, { method: 'DELETE' })
    mcpServers.value = mcpServers.value.filter((s) => s.id !== id)
  }

  // ---- Providers ----
  const providers = ref<LlmProvider[]>([])

  async function loadProviders() {
    providers.value = await api<LlmProvider[]>('/api/assistant/providers')
  }

  async function createProvider(input: LlmProviderInput) {
    const created = await api<LlmProvider>('/api/assistant/providers', {
      method: 'POST',
      body: JSON.stringify(input),
    })
    await loadProviders()
    return created
  }

  async function updateProvider(id: number, input: Partial<LlmProviderInput>) {
    const updated = await api<LlmProvider>(`/api/assistant/providers/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(input),
    })
    await loadProviders()
    return updated
  }

  async function deleteProvider(id: number) {
    await apiVoid(`/api/assistant/providers/${id}`, { method: 'DELETE' })
    providers.value = providers.value.filter((p) => p.id !== id)
  }

  // ---- Models ----
  const models = ref<LlmModel[]>([])

  async function loadModels() {
    models.value = await api<LlmModel[]>('/api/assistant/models')
  }

  async function createModel(input: LlmModelInput) {
    const created = await api<LlmModel>('/api/assistant/models', {
      method: 'POST',
      body: JSON.stringify(input),
    })
    await loadModels()
    return created
  }

  async function updateModel(id: number, input: Partial<LlmModelInput>) {
    const updated = await api<LlmModel>(`/api/assistant/models/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(input),
    })
    await loadModels()
    return updated
  }

  async function deleteModel(id: number) {
    await apiVoid(`/api/assistant/models/${id}`, { method: 'DELETE' })
    models.value = models.value.filter((m) => m.id !== id)
  }

  // ---- Tool permissions ----
  const permissions = ref<ToolPermission[]>([])

  async function loadPermissions() {
    permissions.value = await api<ToolPermission[]>('/api/assistant/permissions')
  }

  async function createPermission(input: ToolPermissionInput) {
    const created = await api<ToolPermission>('/api/assistant/permissions', {
      method: 'POST',
      body: JSON.stringify(input),
    })
    await loadPermissions()
    return created
  }

  async function updatePermission(id: number, input: Partial<ToolPermissionInput>) {
    const updated = await api<ToolPermission>(`/api/assistant/permissions/${id}`, {
      method: 'PATCH',
      body: JSON.stringify(input),
    })
    await loadPermissions()
    return updated
  }

  async function deletePermission(id: number) {
    await apiVoid(`/api/assistant/permissions/${id}`, { method: 'DELETE' })
    permissions.value = permissions.value.filter((p) => p.id !== id)
  }

  return {
    sessions,
    current,
    sending,
    live,
    loadSessions,
    createSession,
    loadSession,
    updateSession,
    deleteSession,
    sendMessage,
    approveToolCalls,
    mcpServers,
    discovered,
    loadMcpServers,
    createMcpServer,
    updateMcpServer,
    deleteMcpServer,
    providers,
    loadProviders,
    createProvider,
    updateProvider,
    deleteProvider,
    models,
    loadModels,
    createModel,
    updateModel,
    deleteModel,
    permissions,
    loadPermissions,
    createPermission,
    updatePermission,
    deletePermission,
  }
})
