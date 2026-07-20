<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { marked } from 'marked'
import { useAssistantStore } from '../stores/assistant'
import type { AssistantMessage } from '../types'

defineProps<{ message: AssistantMessage }>()

const assistant = useAssistantStore()

marked.setOptions({ breaks: true, gfm: true })

interface ToolCall {
  id: string
  name: string
  args: any
}

function renderMarkdown(text: string): string {
  if (!text) return ''
  return marked.parse(text) as string
}

function messageText(content: any): string {
  if (!content) return ''
  if (typeof content === 'string') return content
  if (typeof content.text === 'string') return content.text
  if ('text' in content || 'tool_calls' in content || 'decisions' in content) return ''
  return JSON.stringify(content)
}

function toolCalls(content: any): ToolCall[] {
  if (!content || !Array.isArray(content.tool_calls)) return []
  return content.tool_calls.map((tc: any) => ({
    id: tc.id ?? '',
    name: tc.name,
    args: tc.args,
  }))
}

function toolResult(content: any): { tool_call_id?: string; output?: any; is_error?: boolean } {
  return content || {}
}

function requiresApproval(content: any): boolean {
  return Boolean(content?.requires_approval)
}

function decisionFor(content: any, callId: string): boolean | undefined {
  const arr = Array.isArray(content?.decisions) ? content.decisions : []
  const found = arr.find((d: any) => d.tool_call_id === callId)
  return found ? !!found.approve : undefined
}

async function decide(messageId: number, callId: string, approve: boolean, remember = false) {
  if (!assistant.current) return
  await assistant.approveToolCalls(assistant.current.id, messageId, [
    { tool_call_id: callId, approve, remember },
  ])
}

async function decideAll(messageId: number, calls: ToolCall[], approve: boolean, remember = false) {
  if (!assistant.current) return
  await assistant.approveToolCalls(
    assistant.current.id,
    messageId,
    calls.map((c) => ({ tool_call_id: c.id, approve, remember })),
  )
}

</script>



<template>

  <div v-if="message.role === 'user'" class="flex justify-end">
    <div class="max-w-full whitespace-pre-wrap rounded-lg px-3 py-2 bg-blue-600 text-white text-sm">
      {{ messageText(message.content) }}
    </div>
  </div>
  <div v-else-if="message.role === 'assistant'" class="space-y-1">
    <div
      v-if="messageText(message.content)"
      class="assistant-markdown max-w-full rounded-lg px-3 py-2 bg-gray-100 text-gray-900"
      v-html="renderMarkdown(messageText(message.content))"
    ></div>
    <div
      v-for="tc in toolCalls(message.content)"
      :key="tc.id"
      class="text-xs border-l-2 pl-2 ml-2 font-mono space-y-1"
      :class="
        decisionFor(message.content, tc.id) === false
          ? 'border-red-300 text-red-700'
          : decisionFor(message.content, tc.id) === true
          ? 'border-emerald-300 text-emerald-700'
          : 'border-amber-300 text-gray-500'
      "
    >
      <div>→ {{ tc.name }}({{ JSON.stringify(tc.args) }})</div>
      <div
        v-if="requiresApproval(message.content) && decisionFor(message.content, tc.id) === undefined"
        class="flex gap-2 not-italic flex-wrap"
      >
        <button
          class="px-2 py-0.5 rounded bg-emerald-600 text-white text-xs hover:bg-emerald-500"
          :disabled="assistant.sending"
          @click="decide(message.id, tc.id, true)"
        >Approve</button>
        <button
          class="px-2 py-0.5 rounded border border-emerald-600 text-emerald-700 text-xs hover:bg-emerald-50"
          :disabled="assistant.sending"
          :title="`Always allow ${tc.name} — creates a permission rule`"
          @click="decide(message.id, tc.id, true, true)"
        >Always allow</button>
        <button
          class="px-2 py-0.5 rounded bg-red-600 text-white text-xs hover:bg-red-500"
          :disabled="assistant.sending"
          @click="decide(message.id, tc.id, false)"
        >Reject</button>
        <button
          class="px-2 py-0.5 rounded border border-red-600 text-red-700 text-xs hover:bg-red-50"
          :disabled="assistant.sending"
          :title="`Always reject ${tc.name} — creates a deny rule`"
          @click="decide(message.id, tc.id, false, true)"
        >Always reject</button>
      </div>
    </div>
    <div
      v-if="requiresApproval(message.content) && toolCalls(message.content).length > 1"
      class="ml-2 mt-1 flex gap-2 flex-wrap"
    >
      <button
        class="text-xs px-2 py-0.5 rounded border border-emerald-600 text-emerald-700 hover:bg-emerald-50"
        :disabled="assistant.sending"
        @click="decideAll(message.id, toolCalls(message.content), true)"
      >Approve all</button>
      <button
        class="text-xs px-2 py-0.5 rounded border border-emerald-700 text-emerald-800 hover:bg-emerald-50"
        :disabled="assistant.sending"
        title="Always allow every tool in this batch — creates permission rules"
        @click="decideAll(message.id, toolCalls(message.content), true, true)"
      >Always allow all</button>
      <button
        class="text-xs px-2 py-0.5 rounded border border-red-600 text-red-700 hover:bg-red-50"
        :disabled="assistant.sending"
        @click="decideAll(message.id, toolCalls(message.content), false)"
      >Reject all</button>
      <button
        class="text-xs px-2 py-0.5 rounded border border-red-700 text-red-800 hover:bg-red-50"
        :disabled="assistant.sending"
        title="Always reject every tool in this batch — creates deny rules"
        @click="decideAll(message.id, toolCalls(message.content), false, true)"
      >Always reject all</button>
    </div>
  </div>
  <div v-else-if="message.role === 'tool_result'" class="text-xs ml-2">
    <details
      :open="toolResult(message.content).is_error"
      class="border-l-2 pl-2 font-mono whitespace-pre-wrap"
      :class="toolResult(message.content).is_error ? 'border-red-400 text-red-700' : 'border-emerald-400 text-gray-600'"
    >
      <summary class="cursor-pointer">
        {{ toolResult(message.content).is_error ? '✗ tool error' : '✓ tool result' }}
      </summary>
      <pre class="mt-1">{{ messageText(toolResult(message.content).output) }}</pre>
    </details>
  </div>
  <div v-else-if="message.role === 'error'" class="text-sm text-red-600">
    error: {{ messageText(message.content) }}
  </div>

</template>