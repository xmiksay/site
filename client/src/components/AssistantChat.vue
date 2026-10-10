<script setup lang="ts">
// The open session's transcript, live turn and composer — shared by
// `AssistantView` and the Design studio's chat panel (#119). Reads
// `assistant.current` straight from the store like the other assistant
// components; the parent renders the header and the empty state.
import { computed, nextTick, ref, watch } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import { renderMarkdown } from '../composables/useMarkdown'
import AssistantMessageContent from './AssistantMessageContent.vue'
import LiveToolCallList from './LiveToolCallList.vue'
import LiveSubAgentTurnCard from './LiveSubAgentTurn.vue'

withDefaults(defineProps<{ placeholder?: string }>(), {
  placeholder: 'Type a message…  (Cmd+Enter to send)',
})
const emit = defineEmits<{ 'select-session': [id: number] }>()

const assistant = useAssistantStore()
const draft = ref('')
const messageBox = ref<HTMLDivElement | null>(null)

async function send() {
  const text = draft.value.trim()
  if (!text || !assistant.current) return
  draft.value = ''
  await assistant.sendMessage(assistant.current.id, text)
  scrollToBottom()
}

/** Appends `text` to the composer (e.g. a note about an uploaded asset). */
function insert(text: string) {
  draft.value = draft.value.trim() ? `${draft.value.trimEnd()}\n${text}` : text
}
defineExpose({ insert })

function scrollToBottom() {
  nextTick(() => {
    if (messageBox.value) {
      messageBox.value.scrollTop = messageBox.value.scrollHeight
    }
  })
}

watch(() => [assistant.current?.id, assistant.current?.messages.length], scrollToBottom)

const messageList = computed(() => assistant.current?.messages ?? [])

// The turn streaming live over WS for the open session, if any — see
// `LiveTurn`'s doc in types.ts. `null` once it settles (`done`/`error`),
// at which point `messageList` (from the REST refetch `stores/assistant.ts`
// triggers) is the authoritative view again.
const liveTurn = computed(() => {
  const turn = assistant.live
  return turn && assistant.current?.id === turn.sessionId ? turn : null
})

watch(() => [liveTurn.value?.text, liveTurn.value?.toolCalls.length], scrollToBottom)

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

watch(
  () => liveSubAgentsForCurrent.value.map((t) => t.text.length + t.toolCalls.length).join(','),
  scrollToBottom,
)
</script>

<template>
  <div class="flex-1 flex flex-col min-h-0">
    <div ref="messageBox" class="flex-1 overflow-y-auto p-4 space-y-3">
      <AssistantMessageContent
        v-for="m in messageList"
        :key="m.id"
        :role="m.role"
        :content="m.content"
        :message-id="m.id"
        @decided="scrollToBottom"
        @select-session="emit('select-session', $event)"
      />
      <div v-if="liveTurn" class="space-y-1">
        <div
          v-if="liveTurn.retrying"
          class="inline-flex items-center gap-1 rounded-full bg-amber-100 text-amber-800 text-xs px-2 py-0.5"
        >
          model stalled — retrying…
        </div>
        <div
          v-if="liveTurn.reasoning"
          class="max-w-2xl rounded-lg px-3 py-2 bg-gray-50 text-gray-500 text-xs italic whitespace-pre-wrap"
        >
          {{ liveTurn.reasoning }}
        </div>
        <div
          v-if="liveTurn.text"
          class="assistant-markdown max-w-2xl rounded-lg px-3 py-2 bg-gray-100 text-gray-900"
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
      <div v-if="assistant.sending && !liveTurn" class="text-xs text-gray-500">thinking…</div>
    </div>

    <footer v-if="assistant.current" class="p-3 border-t">
      <form class="flex gap-2" @submit.prevent="send">
        <textarea
          v-model="draft"
          rows="2"
          class="flex-1 border rounded p-2 text-sm"
          :placeholder="placeholder"
          :disabled="assistant.sending"
          @keydown.meta.enter.prevent="send"
          @keydown.ctrl.enter.prevent="send"
        ></textarea>
        <button
          type="submit"
          class="rounded bg-gray-800 hover:bg-gray-700 text-white px-4 py-2 text-sm disabled:opacity-50"
          :disabled="assistant.sending || draft.trim() === ''"
        >
          Send
        </button>
      </form>
    </footer>
  </div>
</template>
