<script setup lang="ts">
// Renders one `{role, content}` pair from the assistant chat transcript.
// Used both for top-level `AssistantSessionDetail.messages` entries and,
// recursively (via `messageId` threaded straight through — the approve
// endpoint ignores it, see `assistant.approveToolCalls`), for the nested
// `content.sub_agents[].messages` a sub-agent's own turn produces.
import { computed, ref } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import { renderMarkdown } from '../composables/useMarkdown'
import type { AssistantSubAgent } from '../types'
import {
  decisionFor,
  messageText,
  needsDecision,
  profileIcon,
  toolCalls,
  toolResult,
  type ToolCallView,
} from '../composables/useAssistantContent'

const props = defineProps<{
  role: string
  content: any
  messageId: number
}>()

const emit = defineEmits<{ decided: [] }>()

const assistant = useAssistantStore()

// Cast once here rather than in the template — `content` is `any`, and
// `v-for` over an un-narrowed `any` makes vue-tsc infer the index as
// `string | number` instead of `number` (the object-iteration overload).
const subAgents = computed<AssistantSubAgent[]>(() => props.content?.sub_agents ?? [])

// Only the calls that still genuinely need a decision — see `needsDecision`'s
// doc for why this is narrower than "every tool_call in a message flagged
// requires_approval".
const pendingCalls = computed<ToolCallView[]>(() => toolCalls(props.content).filter(needsDecision))

// Per-call in-flight/error state, not the global `assistant.sending` — see
// `LiveToolCallList.vue`'s matching comment. This path is otherwise
// self-contained: a successful `approveToolCalls` replaces `assistant.current`
// with the authoritative REST response, so `needsDecision` naturally stops
// rendering the buttons once that lands — no optimistic local state needed
// here, just not leaving the buttons stuck+silent on failure.
const deciding = ref(new Set<string>())
const errors = ref<Record<string, string>>({})

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : 'failed to submit decision'
}

async function decide(callId: string, approve: boolean, remember = false) {
  if (!assistant.current) return
  deciding.value.add(callId)
  delete errors.value[callId]
  try {
    await assistant.approveToolCalls(assistant.current.id, props.messageId, [
      { tool_call_id: callId, approve, remember },
    ])
    emit('decided')
  } catch (e) {
    errors.value[callId] = errorMessage(e)
  } finally {
    deciding.value.delete(callId)
  }
}

async function decideAll(calls: ToolCallView[], approve: boolean, remember = false) {
  if (!assistant.current) return
  const ids = calls.map((c) => c.id)
  ids.forEach((id) => {
    deciding.value.add(id)
    delete errors.value[id]
  })
  try {
    await assistant.approveToolCalls(
      assistant.current.id,
      props.messageId,
      calls.map((c) => ({ tool_call_id: c.id, approve, remember })),
    )
    emit('decided')
  } catch (e) {
    const msg = errorMessage(e)
    ids.forEach((id) => {
      errors.value[id] = msg
    })
  } finally {
    ids.forEach((id) => deciding.value.delete(id))
  }
}
</script>

<template>
  <div v-if="role === 'user'" class="flex justify-end">
    <div class="max-w-2xl whitespace-pre-wrap rounded-lg px-3 py-2 bg-blue-600 text-white">
      {{ messageText(content) }}
    </div>
  </div>
  <div v-else-if="role === 'assistant'" class="space-y-1">
    <div
      v-if="messageText(content)"
      class="assistant-markdown max-w-2xl rounded-lg px-3 py-2 bg-gray-100 text-gray-900"
      v-html="renderMarkdown(messageText(content))"
    ></div>
    <div
      v-for="tc in toolCalls(content)"
      :key="tc.id"
      class="text-xs border-l-2 pl-2 ml-2 font-mono space-y-1"
      :class="
        decisionFor(content, tc.id) === false
          ? 'border-red-300 text-red-700'
          : decisionFor(content, tc.id) === true || tc.resolved
          ? 'border-emerald-300 text-emerald-700'
          : 'border-amber-300 text-gray-500'
      "
    >
      <div>→ {{ tc.name }}({{ JSON.stringify(tc.args) }})</div>
      <div v-if="needsDecision(tc)" class="flex gap-2 not-italic">
        <button
          class="px-2 py-0.5 rounded bg-emerald-600 text-white text-xs hover:bg-emerald-500"
          :disabled="deciding.has(tc.id)"
          @click="decide(tc.id, true)"
        >
          Approve
        </button>
        <button
          class="px-2 py-0.5 rounded border border-emerald-600 text-emerald-700 text-xs hover:bg-emerald-50"
          :disabled="deciding.has(tc.id)"
          :title="`Always allow ${tc.name} — creates a permission rule`"
          @click="decide(tc.id, true, true)"
        >
          Always allow
        </button>
        <button
          class="px-2 py-0.5 rounded bg-red-600 text-white text-xs hover:bg-red-500"
          :disabled="deciding.has(tc.id)"
          @click="decide(tc.id, false)"
        >
          Reject
        </button>
        <button
          class="px-2 py-0.5 rounded border border-red-600 text-red-700 text-xs hover:bg-red-50"
          :disabled="deciding.has(tc.id)"
          :title="`Always reject ${tc.name} — creates a deny rule`"
          @click="decide(tc.id, false, true)"
        >
          Always reject
        </button>
      </div>
      <div v-if="errors[tc.id]" class="text-red-600">{{ errors[tc.id] }}</div>
    </div>
    <div v-if="pendingCalls.length > 1" class="ml-2 mt-1 flex gap-2">
      <button
        class="text-xs px-2 py-0.5 rounded border border-emerald-600 text-emerald-700 hover:bg-emerald-50"
        :disabled="pendingCalls.some((c) => deciding.has(c.id))"
        @click="decideAll(pendingCalls, true)"
      >
        Approve all
      </button>
      <button
        class="text-xs px-2 py-0.5 rounded border border-emerald-700 text-emerald-800 hover:bg-emerald-50"
        :disabled="pendingCalls.some((c) => deciding.has(c.id))"
        title="Always allow every tool in this batch — creates permission rules"
        @click="decideAll(pendingCalls, true, true)"
      >
        Always allow all
      </button>
      <button
        class="text-xs px-2 py-0.5 rounded border border-red-600 text-red-700 hover:bg-red-50"
        :disabled="pendingCalls.some((c) => deciding.has(c.id))"
        @click="decideAll(pendingCalls, false)"
      >
        Reject all
      </button>
      <button
        class="text-xs px-2 py-0.5 rounded border border-red-700 text-red-800 hover:bg-red-50"
        :disabled="pendingCalls.some((c) => deciding.has(c.id))"
        title="Always reject every tool in this batch — creates deny rules"
        @click="decideAll(pendingCalls, false, true)"
      >
        Always reject all
      </button>
    </div>
    <details
      v-for="sa in subAgents"
      :key="sa.agent_id"
      class="ml-2 border-l-2 border-gray-200 pl-2"
    >
      <summary class="cursor-pointer text-xs text-gray-500">
        {{ profileIcon(sa.profile) }} {{ sa.profile }}
        <span v-if="sa.task" class="text-gray-400">— {{ sa.task }}</span>
      </summary>
      <div class="mt-1 space-y-1">
        <AssistantMessageContent
          v-for="(sm, j) in sa.messages"
          :key="j"
          :role="sm.role"
          :content="sm.content"
          :message-id="messageId"
          @decided="emit('decided')"
        />
      </div>
    </details>
  </div>
  <div v-else-if="role === 'tool_result'" class="text-xs ml-2">
    <details
      :open="toolResult(content).is_error"
      class="border-l-2 pl-2 font-mono whitespace-pre-wrap"
      :class="toolResult(content).is_error ? 'border-red-400 text-red-700' : 'border-emerald-400 text-gray-600'"
    >
      <summary class="cursor-pointer">
        {{ toolResult(content).is_error ? '✗ tool error' : '✓ tool result' }}
      </summary>
      <pre class="mt-1">{{ messageText(toolResult(content).output) }}</pre>
    </details>
  </div>
  <div v-else-if="role === 'error'" class="text-sm text-red-600">
    error: {{ messageText(content) }}
  </div>
</template>
