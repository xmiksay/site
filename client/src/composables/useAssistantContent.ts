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
  // Per-call, from `src/ai/projection/mod.rs`'s `OpenTurn::flush_into`/
  // `mark_resolved_calls` — precise signals, unlike the message-level
  // `requires_approval` (below) and the `decisions` array, which the
  // engine's own timing can leave incomplete (see `needsDecision`'s doc).
  requiresApproval: boolean
  resolved: boolean
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
    requiresApproval: Boolean(tc.requires_approval),
    resolved: Boolean(tc.resolved),
  }))
}

export function toolResult(content: any): { tool_call_id?: string; output?: any; is_error?: boolean } {
  return content || {}
}

export function decisionFor(content: any, callId: string): boolean | undefined {
  const arr = Array.isArray(content?.decisions) ? content.decisions : []
  const found = arr.find((d: any) => d.tool_call_id === callId)
  return found ? !!found.approve : undefined
}

// Whether `tc` should still show an Allow/Reject prompt. Deliberately keyed
// off the *call's own* `requiresApproval`/`resolved` flags, not the
// message-level `requiresApproval(content)` + "no decision yet" the four
// buttons used to gate on: a batch can freely mix a call that's genuinely
// still awaiting approval with one the policy auto-allowed (no `decisions`
// entry ever recorded for it) or one whose own decision landed on a
// *different* projected message than this one (an engine/projection timing
// quirk, not a bug in this call's own resolution) — either would otherwise
// look permanently "still pending" forever despite already being done. A call
// that never needed approval at all (`requiresApproval` false) never
// prompts, regardless of `decisions`.
export function needsDecision(tc: ToolCallView): boolean {
  return tc.requiresApproval && !tc.resolved
}

export function profileIcon(profile: string): string {
  if (profile === 'researcher') return '🔎'
  if (profile === 'page-writer') return '✎'
  return '🤖'
}
