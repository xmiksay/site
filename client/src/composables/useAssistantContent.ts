// Pure helpers for reading `AssistantMessage.content`'s loosely-typed shape
// (`{ text?, tool_calls?, requires_approval?, decisions?, sub_agents? }` for
// `assistant` messages, `{ tool_call_id, output, is_error }` for
// `tool_result`, or a bare string). Shared by `AssistantMessageContent.vue`
// (used both top-level and recursively for nested sub-agent transcripts) and
// `LiveToolCallList.vue`.

export interface ToolCallView {
  id: string
  name: string
  args: any
}

export function messageText(content: any): string {
  if (!content) return ''
  if (typeof content === 'string') return content
  if (typeof content.text === 'string') return content.text
  if ('text' in content || 'tool_calls' in content || 'decisions' in content) return ''
  return JSON.stringify(content)
}

export function toolCalls(content: any): ToolCallView[] {
  if (!content || !Array.isArray(content.tool_calls)) return []
  return content.tool_calls.map((tc: any) => ({
    id: tc.id ?? '',
    name: tc.name,
    args: tc.args,
  }))
}

export function toolResult(content: any): { tool_call_id?: string; output?: any; is_error?: boolean } {
  return content || {}
}

export function requiresApproval(content: any): boolean {
  return Boolean(content?.requires_approval)
}

export function decisionFor(content: any, callId: string): boolean | undefined {
  const arr = Array.isArray(content?.decisions) ? content.decisions : []
  const found = arr.find((d: any) => d.tool_call_id === callId)
  return found ? !!found.approve : undefined
}

export function profileIcon(profile: string): string {
  if (profile === 'researcher') return '🔎'
  if (profile === 'page-writer') return '✎'
  return '🤖'
}
