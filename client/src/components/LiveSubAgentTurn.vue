<script setup lang="ts">
// A sub-agent's turn while it's still streaming over WS, before its own
// `done`/`error` clears it from `assistant.liveSubAgents` and the REST
// refetch's `sub_agents` transcript takes over (see `stores/assistant.ts`).
import type { LiveSubAgentTurn } from '../types'
import { renderMarkdown } from '../composables/useMarkdown'
import { profileIcon } from '../composables/useAssistantContent'
import LiveToolCallList from './LiveToolCallList.vue'

defineProps<{ turn: LiveSubAgentTurn }>()
const emit = defineEmits<{ decided: [] }>()
</script>

<template>
  <details open class="ml-2 border-l-2 border-amber-200 pl-2">
    <summary class="cursor-pointer text-xs text-gray-500">
      {{ profileIcon(turn.profile) }} {{ turn.profile || 'sub-agent' }} — running…
    </summary>
    <div class="mt-1 space-y-1">
      <div
        v-if="turn.reasoning"
        class="max-w-2xl rounded-lg px-3 py-2 bg-gray-50 text-gray-500 text-xs italic whitespace-pre-wrap"
      >
        {{ turn.reasoning }}
      </div>
      <div
        v-if="turn.text"
        class="assistant-markdown max-w-2xl rounded-lg px-3 py-2 bg-gray-100 text-gray-900"
        v-html="renderMarkdown(turn.text)"
      ></div>
      <LiveToolCallList
        :tool-calls="turn.toolCalls"
        :session-id="turn.dbSessionId"
        @decided="emit('decided')"
      />
    </div>
  </details>
</template>
