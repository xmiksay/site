<script setup lang="ts">
import { onMounted } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import AssistantChat from '../components/AssistantChat.vue'
import AssistantSessionToolbar from '../components/AssistantSessionToolbar.vue'
import AssistantSessionTree from '../components/AssistantSessionTree.vue'
import { firstRootSession } from '../composables/useSessionTree'

const assistant = useAssistantStore()

onMounted(async () => {
  await Promise.all([
    assistant.loadSessions(),
    assistant.loadModels(),
    assistant.loadPermissions(),
    assistant.loadMcpServers(),
  ])
  // The first *root*, not `sessions[0]`: since #99 a sub-agent is a session
  // row too, so the API's first entry can be a child — opening one by default
  // would land the user in a read-only sub-transcript.
  const first = firstRootSession(assistant.sessions)
  if (first) await select(first.id)
})

async function newSession() {
  if (assistant.models.length === 0) {
    alert('Add a provider and a model first under "LLM providers" / "LLM models".')
    return
  }
  const s = await assistant.createSession()
  await select(s.id)
}

async function select(id: number) {
  await assistant.loadSession(id)
}

async function deleteSession(id: number) {
  if (!confirm('Delete this chat?')) return
  await assistant.deleteSession(id)
  if (!assistant.current) {
    const next = firstRootSession(assistant.sessions)
    if (next) await select(next.id)
  }
}

async function updateTitle() {
  if (!assistant.current) return
  const newTitle = prompt('New title', assistant.current.title)
  if (newTitle && newTitle !== assistant.current.title) {
    await assistant.updateSession(assistant.current.id, { title: newTitle })
    if (assistant.current) assistant.current.title = newTitle
  }
}
</script>

<template>
  <div class="flex h-[calc(100vh-8rem)] md:h-[calc(100vh-3rem)] gap-4">
    <aside
      class="w-full md:w-64 bg-white rounded-lg shadow flex-col"
      :class="assistant.current ? 'hidden md:flex' : 'flex'"
    >
      <div class="p-3 border-b flex items-center justify-between">
        <h2 class="font-semibold">Chats</h2>
        <button
          class="text-sm rounded bg-gray-800 hover:bg-gray-700 text-white px-2 py-1"
          @click="newSession"
        >
          New
        </button>
      </div>
      <AssistantSessionTree
        :sessions="assistant.sessions"
        :current-id="assistant.current?.id ?? null"
        @select="select"
        @delete="deleteSession"
      />
    </aside>

    <section
      class="flex-1 bg-white rounded-lg shadow flex-col min-w-0"
      :class="!assistant.current ? 'hidden md:flex' : 'flex'"
    >
      <header v-if="assistant.current" class="p-3 border-b flex items-center justify-between gap-2">
        <div class="flex items-center gap-2 min-w-0">
          <button
            type="button"
            class="md:hidden p-1 rounded hover:bg-gray-100 text-gray-600"
            aria-label="Back to chats"
            @click="assistant.current = null"
          >
            <svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7" />
            </svg>
          </button>
          <button
            class="text-left hover:underline truncate font-semibold"
            @click="updateTitle"
            :title="assistant.current.title"
          >
            {{ assistant.current.title }}
          </button>
        </div>
        <AssistantSessionToolbar />
      </header>

      <AssistantChat v-if="assistant.current" @select-session="select" />

      <div v-if="!assistant.current" class="flex-1 flex items-center justify-center text-gray-500">
        Pick a chat or start a new one.
      </div>
    </section>
  </div>
</template>
