<script setup lang="ts">
import { onMounted } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import ChatPanel from '../components/ChatPanel.vue'
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
</script>

<template>
  <div class="flex h-[calc(100vh-8rem)] md:h-[calc(100vh-3rem)] gap-4">
    <aside
      class="w-full md:w-64 bg-surface rounded-lg shadow flex-col"
      :class="assistant.current ? 'hidden md:flex' : 'flex'"
    >
      <div class="p-3 border-b flex items-center justify-between">
        <h2 class="font-semibold">Chats</h2>
        <button
          class="text-sm rounded button-primary px-2 py-1"
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
      class="flex-1 min-w-0 flex flex-col"
      :class="!assistant.current ? 'hidden md:flex' : 'flex'"
    >
      <ChatPanel
        v-if="assistant.current"
        class="flex-1 bg-surface rounded-lg shadow overflow-hidden"
        :show-back="true"
        @back="assistant.current = null"
      />
      <div v-else class="flex-1 flex items-center justify-center text-fg-3">
        Pick a chat or start a new one.
      </div>
    </section>
  </div>
</template>
