<script setup lang="ts">
// Renders one `{role, content}` pair from the assistant chat transcript.
// No longer self-recursive: since #100 a spawned sub-agent contributes a
// reference card (`content.sub_agents[]`, no `messages`) instead of a nested
// transcript, and its own messages are rendered by opening that session.
import { computed, ref } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import { renderMarkdown } from '../composables/useMarkdown'
import type { AssistantSubAgent } from '../types'
import { attachmentUrl, splitAttachments } from '../lib/chatAttachments'
import { isImagePath } from '../lib/designPaths'
import {
  decisionFor,
  messageReasoning,
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

// `selectSession` carries a sub-agent card's `child_db_session_id` up to
// `AssistantView`, which owns session selection — the card never loads a
// session itself.
const emit = defineEmits<{ decided: []; selectSession: [id: number] }>()

const assistant = useAssistantStore()

// Cast once here rather than in the template — `content` is `any`, and
// `v-for` over an un-narrowed `any` makes vue-tsc infer the index as
// `string | number` instead of `number` (the object-iteration overload).
const subAgents = computed<AssistantSubAgent[]>(() => props.content?.sub_agents ?? [])

// A user message's trailing attachment note renders as links (#132).
const userMessage = computed(() => splitAttachments(messageText(props.content)))

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
    <div class="max-w-2xl whitespace-pre-wrap rounded-lg px-3 py-2 bg-accent text-fg-inverse">
      {{ userMessage.body }}
      <ul v-if="userMessage.paths.length" class="mt-2 flex flex-wrap gap-2 whitespace-normal" aria-label="Attachments">
        <li v-for="path in userMessage.paths" :key="path">
          <span v-if="!attachmentUrl(path)" class="text-xs">{{ path }}</span>
          <a
            v-else
            :href="attachmentUrl(path)!"
            target="_blank"
            rel="noopener"
            class="inline-flex items-center gap-1 text-xs underline"
            :title="path"
          >
            <img
              v-if="isImagePath(path)"
              :src="attachmentUrl(path, true) ?? undefined"
              alt=""
              class="h-12 w-12 rounded object-cover bg-surface"
              @error="($event.target as HTMLImageElement).hidden = true"
            />
            {{ path.slice(path.lastIndexOf('/') + 1) }}
          </a>
        </li>
      </ul>
    </div>
  </div>
  <div v-else-if="role === 'assistant'" class="space-y-1">
    <!-- Closed by default: thinking is context for a curious reader, not the
         answer, so it must never push the reply itself off screen. Styled to
         match the live reasoning bubble in `AssistantView.vue`. -->
    <details
      v-if="messageReasoning(content)"
      class="max-w-2xl rounded-lg px-3 py-2 bg-surface-alt text-fg-3 text-xs italic"
    >
      <summary class="cursor-pointer not-italic">Thinking</summary>
      <div class="mt-1 whitespace-pre-wrap">{{ messageReasoning(content) }}</div>
    </details>
    <div
      v-if="messageText(content)"
      class="assistant-markdown max-w-2xl rounded-lg px-3 py-2 bg-surface-raised text-fg-1"
      v-html="renderMarkdown(messageText(content))"
    ></div>
    <div
      v-for="tc in toolCalls(content)"
      :key="tc.id"
      class="text-xs border-l-2 pl-2 ml-2 font-mono space-y-1"
      :class="
        decisionFor(content, tc.id) === false
          ? 'border-danger-soft text-danger-strong'
          : decisionFor(content, tc.id) === true || tc.resolved
          ? 'border-success-soft text-success-strong'
          : 'border-warning-soft text-fg-3'
      "
    >
      <div>→ {{ tc.name }}({{ JSON.stringify(tc.args) }})</div>
      <div v-if="needsDecision(tc)" class="flex gap-2 not-italic">
        <button
          class="px-2 py-0.5 rounded button-success text-xs"
          :disabled="deciding.has(tc.id)"
          @click="decide(tc.id, true)"
        >
          Approve
        </button>
        <button
          class="px-2 py-0.5 rounded button-outline-success text-xs"
          :disabled="deciding.has(tc.id)"
          :title="`Always allow ${tc.name} — creates a permission rule`"
          @click="decide(tc.id, true, true)"
        >
          Always allow
        </button>
        <button
          class="px-2 py-0.5 rounded button-danger text-xs"
          :disabled="deciding.has(tc.id)"
          @click="decide(tc.id, false)"
        >
          Reject
        </button>
        <button
          class="px-2 py-0.5 rounded button-outline-danger text-xs"
          :disabled="deciding.has(tc.id)"
          :title="`Always reject ${tc.name} — creates a deny rule`"
          @click="decide(tc.id, false, true)"
        >
          Always reject
        </button>
      </div>
      <div v-if="errors[tc.id]" class="text-danger">{{ errors[tc.id] }}</div>
    </div>
    <div v-if="pendingCalls.length > 1" class="ml-2 mt-1 flex gap-2">
      <button
        class="text-xs px-2 py-0.5 rounded border border-success text-success-strong hover:bg-success-bg"
        :disabled="pendingCalls.some((c) => deciding.has(c.id))"
        @click="decideAll(pendingCalls, true)"
      >
        Approve all
      </button>
      <button
        class="text-xs px-2 py-0.5 rounded border border-success text-success-strong hover:bg-success-bg"
        :disabled="pendingCalls.some((c) => deciding.has(c.id))"
        title="Always allow every tool in this batch — creates permission rules"
        @click="decideAll(pendingCalls, true, true)"
      >
        Always allow all
      </button>
      <button
        class="text-xs px-2 py-0.5 rounded border border-danger text-danger-strong hover:bg-danger-bg"
        :disabled="pendingCalls.some((c) => deciding.has(c.id))"
        @click="decideAll(pendingCalls, false)"
      >
        Reject all
      </button>
      <button
        class="text-xs px-2 py-0.5 rounded border border-danger text-danger-strong hover:bg-danger-bg"
        :disabled="pendingCalls.some((c) => deciding.has(c.id))"
        title="Always reject every tool in this batch — creates deny rules"
        @click="decideAll(pendingCalls, false, true)"
      >
        Always reject all
      </button>
    </div>
    <!-- A sub-agent is its own session (#99), so this is a reference card, not
         a nested transcript: clicking it opens the child's own session (#103).
         `child_db_session_id` is absent when the server never placed the
         child's row, so the card renders flat rather than as a dead link — a
         plain `div`, no hover affordance, no click. -->
    <component
      :is="sa.child_db_session_id != null ? 'button' : 'div'"
      v-for="sa in subAgents"
      :key="sa.agent_id"
      :type="sa.child_db_session_id != null ? 'button' : undefined"
      class="ml-2 border-l-2 border-line-2 pl-2 py-1 text-xs text-fg-3 space-y-0.5 block w-full text-left"
      :class="
        sa.child_db_session_id != null
          ? 'cursor-pointer hover:border-line-strong hover:bg-surface-alt'
          : ''
      "
      :title="sa.child_db_session_id != null ? 'Open this sub-agent’s chat' : undefined"
      @click="sa.child_db_session_id != null && emit('selectSession', sa.child_db_session_id)"
    >
      <div>
        {{ profileIcon(sa.profile) }} {{ sa.profile }}
        <span v-if="sa.task" class="text-fg-4">— {{ sa.task }}</span>
        <span class="text-fg-4">({{ sa.message_count }} messages)</span>
      </div>
      <div v-if="sa.preview" class="text-fg-2 italic">{{ sa.preview }}</div>
    </component>
  </div>
  <div v-else-if="role === 'tool_result'" class="text-xs ml-2">
    <details
      :open="toolResult(content).is_error"
      class="border-l-2 pl-2 font-mono whitespace-pre-wrap"
      :class="toolResult(content).is_error ? 'border-danger-soft text-danger-strong' : 'border-success-soft text-fg-2'"
    >
      <summary class="cursor-pointer">
        {{ toolResult(content).is_error ? '✗ tool error' : '✓ tool result' }}
      </summary>
      <pre class="mt-1">{{ messageText(toolResult(content).output) }}</pre>
    </details>
  </div>
  <div v-else-if="role === 'error'" class="text-sm text-danger">
    error: {{ messageText(content) }}
  </div>
</template>
