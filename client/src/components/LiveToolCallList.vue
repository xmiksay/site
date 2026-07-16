<script setup lang="ts">
// Renders in-progress tool calls for a turn still streaming over WS — reused
// for both the root session's own `liveTurn` and each streaming sub-agent's
// `LiveSubAgentTurn` (`sessionId` is always the ROOT chat's DB id in either
// case, since the approve endpoint is keyed by `tool_call_id` alone — see
// `stores/assistant.ts`'s WS handler doc).
import type { LiveToolCall } from '../types'
import { messageText } from '../composables/useAssistantContent'
import { useAssistantStore } from '../stores/assistant'

const props = defineProps<{
  toolCalls: LiveToolCall[]
  sessionId: number
}>()

const emit = defineEmits<{ decided: [] }>()

const assistant = useAssistantStore()

async function decide(callId: string, approve: boolean, remember = false) {
  await assistant.approveToolCalls(props.sessionId, 0, [
    { tool_call_id: callId, approve, remember },
  ])
  emit('decided')
}
</script>

<template>
  <div
    v-for="tc in toolCalls"
    :key="tc.id"
    class="text-xs border-l-2 pl-2 ml-2 font-mono space-y-1"
    :class="tc.status === 'done' ? 'border-emerald-300 text-emerald-700' : 'border-amber-300 text-gray-500'"
  >
    <div>→ {{ tc.name }}({{ tc.argsText }})</div>
    <div v-if="tc.status === 'requires_approval'" class="flex gap-2 not-italic">
      <button
        class="px-2 py-0.5 rounded bg-emerald-600 text-white text-xs hover:bg-emerald-500"
        :disabled="assistant.sending"
        @click="decide(tc.id, true)"
      >
        Approve
      </button>
      <button
        class="px-2 py-0.5 rounded border border-emerald-600 text-emerald-700 text-xs hover:bg-emerald-50"
        :disabled="assistant.sending"
        :title="`Always allow ${tc.name} — creates a permission rule`"
        @click="decide(tc.id, true, true)"
      >
        Always allow
      </button>
      <button
        class="px-2 py-0.5 rounded bg-red-600 text-white text-xs hover:bg-red-500"
        :disabled="assistant.sending"
        @click="decide(tc.id, false)"
      >
        Reject
      </button>
      <button
        class="px-2 py-0.5 rounded border border-red-600 text-red-700 text-xs hover:bg-red-50"
        :disabled="assistant.sending"
        :title="`Always reject ${tc.name} — creates a deny rule`"
        @click="decide(tc.id, false, true)"
      >
        Always reject
      </button>
    </div>
    <div v-else-if="tc.status === 'done'">✓ {{ messageText(tc.output) }}</div>
  </div>
</template>
