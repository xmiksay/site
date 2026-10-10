<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import { renderMarkdown } from '../composables/useMarkdown'
import AssistantMessageContent from './AssistantMessageContent.vue'
import LiveToolCallList from './LiveToolCallList.vue'
import LiveSubAgentTurnCard from './LiveSubAgentTurn.vue'

const assistant = useAssistantStore()
const messageList = computed(() => assistant.current?.messages ?? [])
const messageBox = ref<HTMLDivElement | null>(null)
const error = ref('')

function scrollToBottom() {
  nextTick(() => {
    if (messageBox.value) {
      messageBox.value.scrollTop = messageBox.value.scrollHeight
    }
  })
}

// Opening a sub-agent's card can fail (e.g. the child was deleted); show it
// here rather than leave an unhandled rejection.
async function selectSession(id: number) {
  error.value = ''
  try {
    await assistant.loadSession(id)
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  }
}

watch(
  () => assistant.current?.id,
  () => {
    error.value = ''
  },
)

// The turn streaming live over WS for the open session, if any — see
// `LiveTurn`'s doc in types.ts. `null` once it settles (`done`/`error`),
// at which point `messageList` (from the REST refetch `stores/assistant.ts`
// triggers) is the authoritative view again.
const liveTurn = computed(() => {
  const turn = assistant.live
  return turn && assistant.current?.id === turn.sessionId ? turn : null
})

// Sub-agents (`researcher`/`page-writer`) currently streaming for the open
// session — see `LiveSubAgentTurn`'s doc in types.ts. Filtered the same way
// `liveTurn` is (by the currently open session's id), since a child can
// outlive the root's own `live` turn and keeps its entry independently.
// `dbSessionId` is the **root's** row, so matching `childDbSessionId` too
// (#102) is what keeps the child's *own* session view live instead of blank
// until its turn settles and the REST refetch lands.
const liveSubAgentsForCurrent = computed(() => {
  const openId = assistant.current?.id
  // Explicit null-check: `childDbSessionId` is optional, so a bare
  // `=== assistant.current?.id` would match every card once nothing is open.
  if (openId == null) return []
  return Object.values(assistant.liveSubAgents).filter(
    (turn) => turn.dbSessionId === openId || turn.childDbSessionId === openId,
  )
})

watch(() => assistant.current, scrollToBottom)
watch(() => assistant.current?.messages.length, scrollToBottom)
watch(() => [liveTurn.value?.text, liveTurn.value?.toolCalls.length], scrollToBottom)
watch(
  () => liveSubAgentsForCurrent.value.map((t) => t.text.length + t.toolCalls.length).join(','),
  scrollToBottom,
)
</script>

<template>
  <div ref="messageBox" class="flex-1 overflow-y-auto p-4 space-y-3 min-h-0">
    <p v-if="error" class="text-sm text-danger-strong bg-danger-bg border border-danger-soft rounded p-2">
      {{ error }}
    </p>
    <AssistantMessageContent
      v-for="m in messageList"
      :key="m.id"
      :role="m.role"
      :content="m.content"
      :message-id="m.id"
      @decided="scrollToBottom"
      @select-session="selectSession"
    />
    <div v-if="liveTurn" class="space-y-1">
      <div
        v-if="liveTurn.retrying"
        class="inline-flex items-center gap-1 rounded-full bg-warning-bg text-warning text-xs px-2 py-0.5"
      >
        model stalled — retrying…
      </div>
      <div
        v-if="liveTurn.reasoning"
        class="max-w-2xl rounded-lg px-3 py-2 bg-surface-alt text-fg-3 text-xs italic whitespace-pre-wrap"
      >
        {{ liveTurn.reasoning }}
      </div>
      <div
        v-if="liveTurn.text"
        class="assistant-markdown max-w-2xl rounded-lg px-3 py-2 bg-surface-raised text-fg-1"
        v-html="renderMarkdown(liveTurn.text)"
      ></div>
      <LiveToolCallList
        :tool-calls="liveTurn.toolCalls"
        :session-id="liveTurn.sessionId"
        @decided="scrollToBottom"
      />
    </div>
    <LiveSubAgentTurnCard
      v-for="turn in liveSubAgentsForCurrent"
      :key="turn.agentSessionId"
      :turn="turn"
      @decided="scrollToBottom"
    />
    <div v-if="assistant.sending && !liveTurn" class="text-xs text-fg-3">thinking…</div>
  </div>
</template>
