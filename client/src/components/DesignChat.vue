<script setup lang="ts">
// The Design studio's chat panel (#119): a session under the `designer`
// profile, which edits the shared draft. A chat with history cannot switch
// into `designer` (the API answers 409), so this panel only ever creates new
// Designer chats or resumes earlier ones — never switches the open chat.
// Attaching images and fonts goes through the composer: the server stores a
// Designer chat's attachments in the draft's assets/ (#132).
import { computed, onMounted, ref } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import ChatPanel from './ChatPanel.vue'

const assistant = useAssistantStore()
const error = ref('')

const designerSessions = computed(() =>
  assistant.sessions.filter((s) => s.agent_profile === 'designer' && s.parent_session_id == null),
)
const current = computed(() => assistant.current)
// A sub-agent's transcript opened from a card in the chat; read-only.
const isChild = computed(() => current.value?.parent_session_id != null)

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

async function select(id: number) {
  error.value = ''
  try {
    await assistant.loadSession(id)
  } catch (e) {
    error.value = message(e)
  }
}

async function newChat() {
  if (assistant.models.length === 0) {
    error.value = 'Add a provider and a model first under "LLM providers" / "LLM models".'
    return
  }
  error.value = ''
  try {
    const s = await assistant.createSession({ title: 'Design', agent_profile: 'designer' })
    await assistant.loadSession(s.id)
  } catch (e) {
    error.value = message(e)
  }
}

onMounted(async () => {
  try {
    await Promise.all([assistant.loadSessions(), assistant.loadModels(), assistant.loadMcpServers()])
  } catch (e) {
    error.value = message(e)
    return
  }
  if (current.value?.agent_profile === 'designer') return
  const first = designerSessions.value[0]
  if (first) await select(first.id)
  else assistant.current = null
})
</script>

<template>
  <div class="flex flex-col min-h-0 flex-1">
    <div class="p-2 border-b border-line-2 flex flex-wrap items-center gap-2 text-sm">
      <select
        v-if="designerSessions.length"
        :value="isChild ? current?.parent_session_id : current?.id"
        class="min-w-0 flex-1 rounded border border-line-1 bg-surface px-2 py-1"
        aria-label="Designer chats"
        @change="select(Number(($event.target as HTMLSelectElement).value))"
      >
        <option v-for="s in designerSessions" :key="s.id" :value="s.id">
          {{ s.title }} · {{ new Date(s.updated_at).toLocaleDateString() }}
        </option>
      </select>
      <button type="button" class="rounded button-primary px-2 py-1" @click="newChat">
        New Designer chat
      </button>
    </div>
    <p v-if="error" class="m-2 text-sm text-danger-strong bg-danger-bg border border-danger-soft rounded p-2">
      {{ error }}
    </p>
    <ChatPanel
      v-if="current"
      class="flex-1"
      placeholder="Describe the design change…  (Cmd+Enter to send)"
    >
      <template #actions>
        <button
          v-if="isChild"
          type="button"
          class="text-xs text-accent hover:underline"
          @click="select(current.parent_session_id!)"
        >
          ← Back to the Designer chat
        </button>
      </template>
    </ChatPanel>
    <div v-else class="flex-1 flex items-center justify-center p-4 text-center text-sm text-fg-3">
      Start a Designer chat: the assistant edits the draft, the preview shows the result.
    </div>
  </div>
</template>
