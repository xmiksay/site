export type WsTopic = 'assistant' | 'pages' | 'files' | 'galleries' | 'tags'

export interface WsEnvelope<T = any> {
  topic: WsTopic
  event: string
  payload: T
}

export interface PageSummary {
  id: number
  path: string
  summary: string | null
  tag_ids: number[]
  private: boolean
  created_at: string
  modified_at: string
}

export interface PageDetail extends PageSummary {
  markdown: string
  revisions: Array<{ id: number; created_at: string }>
}

export interface RevisionDetail {
  id: number
  created_at: string
  markdown: string
  diff: string
}

export interface PageInput {
  path: string
  summary: string | null
  markdown: string
  tag_ids: number[]
  private: boolean
}

export interface Tag {
  id: number
  name: string
  description: string | null
}

export interface FileSummary {
  id: number
  hash: string
  path: string
  title: string
  description: string | null
  mimetype: string
  size_bytes: number
  has_thumbnail: boolean
  created_at: string
}

export interface Gallery {
  id: number
  path: string
  title: string
  description: string | null
  file_ids: number[]
  created_at: string
  created_by: number
}

export interface MenuItem {
  id: number
  title: string
  path: string
  markdown: string
  order_index: number
  private: boolean
}

export interface TokenSummary {
  id: number
  label: string | null
  is_service: boolean
  expires_at: string | null
}

export interface TokenCreated extends TokenSummary {
  nonce: string
}

export interface UserSummary {
  id: number
  username: string
  is_self: boolean
}

// Assistant
export interface AssistantSession {
  id: number
  title: string
  provider: string
  model: string
  model_id: number | null
  enabled_mcp_server_ids: number[]
  created_at: string
  updated_at: string
}

export interface AssistantMessage {
  id: number
  seq: number
  role: string
  content: any
  created_at: string
}

export interface AssistantSessionDetail extends AssistantSession {
  messages: AssistantMessage[]
}

/**
 * One in-flight tool call surfaced live via `assistant.*` WS events, before
 * it lands in `AssistantSessionDetail.messages` via the next REST refetch.
 */
export interface LiveToolCall {
  id: string
  name: string
  argsText: string
  args: any
  status: 'pending' | 'requires_approval' | 'done'
  output?: string
}

/**
 * The turn currently streaming for one session — accumulated from
 * `text_delta`/`reasoning_delta`/`tool_call*`/`tool_output` envelopes on the
 * `assistant` WS topic. Cleared once the turn settles (`done`/`error`), at
 * which point the authoritative message list comes from a REST refetch.
 */
export interface LiveTurn {
  sessionId: number
  text: string
  reasoning: string
  toolCalls: LiveToolCall[]
}

/**
 * A sub-agent (`researcher`/`page-writer`) spawned mid-turn via an
 * `agent_spawn`/`agent` tool call, as returned in the REST transcript on the
 * assistant message whose `tool_calls` includes that spawn — a sibling of
 * `tool_calls` on `AssistantMessage.content`, not a separate top-level
 * message. The backend matches each entry to its spawning call structurally
 * (via the call's own tool_result, not array position — a batch of several
 * spawns, or an earlier refused spawn, can't misattribute a child), so `task`
 * (copied from that same call's `args.prompt`) is already the right one —
 * never re-derive it by index. `messages` never contains a `role: "user"`
 * entry — that's what `task` is for.
 */
export interface AssistantSubAgent {
  agent_id: string
  profile: string
  task: string
  messages: Array<{ role: string; content: any }>
}

/**
 * A sub-agent's own turn streaming live over the `assistant` WS topic,
 * identified by `agent_session_id` rather than `db_session_id` — kept in its
 * own bucket (keyed by `agentSessionId`, not nested inside `LiveTurn`)
 * because a child keeps running detached after the root's own `live` turn
 * has already settled and cleared.
 */
export interface LiveSubAgentTurn {
  agentSessionId: string
  dbSessionId: number
  profile: string
  text: string
  reasoning: string
  toolCalls: LiveToolCall[]
  done: boolean
}

export interface McpServer {
  id: number
  name: string
  url: string
  enabled: boolean
  forward_user_token: boolean
  headers: Record<string, string>
  /** Raw remote tool name -> capability (`read`/`write`/`call`), for capability fan-out (#39). */
  capabilities: Record<string, string>
  created_at: string
}

export interface McpDiscoveredTool {
  name: string
  prefixed_name: string
  description: string
  schema: any
}

export interface McpDiscoveredServer {
  name: string
  url: string
  enabled: boolean
  forward_user_token: boolean
  connected: boolean
  tools: McpDiscoveredTool[]
}

export interface McpServerListResponse {
  user_servers: McpServer[]
  discovered: McpDiscoveredServer[]
}

export interface LlmProvider {
  id: number
  label: string
  kind: 'anthropic' | 'ollama' | 'gemini' | string
  base_url: string | null
  has_api_key: boolean
  /** Max concurrent in-flight requests for this provider; `null` uses the engine default (3). */
  concurrency: number | null
  /** Requests/minute cap for this provider; `null` uses the engine default (50). */
  rpm: number | null
  created_at: string
}

export interface LlmProviderInput {
  label: string
  kind: string
  api_key?: string
  base_url?: string
  /** `null` clears it back to the engine default; omit to leave untouched on a patch. */
  concurrency?: number | null
  /** `null` clears it back to the engine default; omit to leave untouched on a patch. */
  rpm?: number | null
}

export interface LlmModel {
  id: number
  provider_id: number
  provider_label: string
  provider_kind: string
  label: string
  model: string
  is_default: boolean
  /** Real context window in tokens (#40); `null` falls back to the engine's generic default. */
  context_window: number | null
  created_at: string
}

export interface LlmModelInput {
  provider_id: number
  label: string
  model: string
  is_default?: boolean
  context_window?: number
}

export interface ToolPermission {
  id: number
  name: string
  effect: 'allow' | 'deny' | 'prompt' | string
  priority: number
  created_at: string
}

export interface ToolPermissionInput {
  name: string
  effect: string
  priority?: number
}
