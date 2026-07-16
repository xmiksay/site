export type WsTopic = 'assistant' | 'pages' | 'files' | 'galleries'

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

export interface McpServer {
  id: number
  name: string
  url: string
  enabled: boolean
  forward_user_token: boolean
  headers: Record<string, string>
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
  created_at: string
}

export interface LlmProviderInput {
  label: string
  kind: string
  api_key?: string
  base_url?: string
}

export interface LlmModel {
  id: number
  provider_id: number
  provider_label: string
  provider_kind: string
  label: string
  model: string
  is_default: boolean
  created_at: string
}

export interface LlmModelInput {
  provider_id: number
  label: string
  model: string
  is_default?: boolean
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
