import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, apiVoid } from '../api'
import { useLiveTurns } from './assistantLiveTurns'
import type {
  AssistantSession,
  AssistantSessionDetail,
  LlmModel,
  LlmModelInput,
  LlmProvider,
  LlmProviderInput,
  McpServer,
  McpServerListResponse,
  ProviderThrottleStatus,
  ToolPermission,
  ToolPermissionInput,
} from '../types'

export const useAssistantStore = defineStore('assistant', () => {
  const sessions = ref<AssistantSession[]>([])
  const current = ref<AssistantSessionDetail | null>(null)
  const sending = ref(false)

  // `live`/`liveSubAgents` fold the `assistant` WS topic into in-progress
  // turn state — see `assistantLiveTurns.ts`'s doc. `loadSession` is a
  // (hoisted) function declaration, so passing it here ahead of its own
  // definition further down is fine — the whole store's setup body runs
  // synchronously before anything can call into `useLiveTurns`'s WS handler.
  const { live, liveSubAgents, resolveLiveToolCall } = useLiveTurns(current, sending, loadSession)

  async function loadSessions() {
    sessions.value = await api<AssistantSession[]>('/api/assistant/sessions')
  }

  async function createSession(input: {
    title?: string
    model_id?: number
    enabled_mcp_server_ids?: number[]
    temperature?: number | null
    reasoning_effort?: string | null
    max_output_tokens?: number | null
    thinking_budget_tokens?: number | null
    agent_profile?: string
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
    input: {
      title?: string
      model_id?: number
      enabled_mcp_server_ids?: number[]
      temperature?: number | null
      reasoning_effort?: string | null
      max_output_tokens?: number | null
      thinking_budget_tokens?: number | null
      agent_profile?: string
    },
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

  async function compactSession(
    id: number,
    input: { instructions?: string; kept?: number } = {},
  ) {
    sending.value = true
    try {
      current.value = await api<AssistantSessionDetail>(
        `/api/assistant/sessions/${id}/compact`,
        { method: 'POST', body: JSON.stringify(input) },
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
    capabilities?: Record<string, string>
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
      capabilities?: Record<string, string>
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

  const throttleStatuses = ref<ProviderThrottleStatus[]>([])

  async function loadThrottleStatuses() {
    throttleStatuses.value = await api<ProviderThrottleStatus[]>('/api/assistant/providers/status')
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
    liveSubAgents,
    resolveLiveToolCall,
    loadSessions,
    createSession,
    loadSession,
    updateSession,
    deleteSession,
    sendMessage,
    compactSession,
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
    throttleStatuses,
    loadThrottleStatuses,
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
