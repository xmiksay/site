<script setup lang="ts">
import { ref } from 'vue'
import { useAssistantStore } from '../stores/assistant'

const props = defineProps<{
  showBack?: boolean
}>()

const assistant = useAssistantStore()
const emit = defineEmits<{ back: [] }>()

async function changeModel(modelId: number) {
  if (!assistant.current) return
  await assistant.updateSession(assistant.current.id, { model_id: modelId })
  await assistant.loadSession(assistant.current.id)
}

async function updateTitle() {
  if (!assistant.current) return
  const newTitle = prompt('New title', assistant.current.title)
  if (newTitle && newTitle !== assistant.current.title) {
    await assistant.updateSession(assistant.current.id, { title: newTitle })
    if (assistant.current) assistant.current.title = newTitle
  }
}

async function toggleMcpServer(serverId: number, on: boolean) {
  if (!assistant.current) return
  const current = assistant.current.enabled_mcp_server_ids ?? []
  const next = on
    ? Array.from(new Set([...current, serverId]))
    : current.filter((id) => id !== serverId)
  await assistant.updateSession(assistant.current.id, { enabled_mcp_server_ids: next })
  await assistant.loadSession(assistant.current.id)
}

const showMcpPicker = ref(false)


</script>

<template>
  <header v-if="assistant.current" class="shrink-0 p-3 border-b flex items-center justify-between gap-2">
    <div class="flex items-center gap-2 min-w-0">
      <button
        v-if="showBack"
        type="button"
        class="md:hidden p-1 rounded hover:bg-gray-100 text-gray-600 shrink-0"
        aria-label="Back to chats"
        @click="emit('back')"
      >
        <svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7" />
        </svg>
      </button>
      <button
        class="text-left hover:underline truncate font-semibold text-sm"
        @click="updateTitle"
        :title="assistant.current.title"
      >
        {{ assistant.current.title }}
      </button>
    </div>
    <div class="flex items-center gap-2 shrink-0">
      <select
        class="border rounded px-2 py-1 text-xs"
        :value="assistant.current.model_id ?? ''"
        @change="changeModel(Number(($event.target as HTMLSelectElement).value))"
      >
        <option v-if="!assistant.current.model_id" :value="''" disabled>
          {{ assistant.current.provider }} / {{ assistant.current.model }}
        </option>
        <option v-for="m in assistant.models" :key="m.id" :value="m.id">
          {{ m.label }}
        </option>
      </select>
      <div class="relative">
        <button
          type="button"
          class="border rounded px-2 py-1 text-xs hover:bg-gray-50"
          @click="showMcpPicker = !showMcpPicker"
          title="MCP servers active in this chat"
        >
          MCP
          <span class="ml-1 inline-block min-w-[1rem] text-center rounded bg-gray-100 px-1">
            {{ (assistant.current.enabled_mcp_server_ids ?? []).length }}/{{ assistant.mcpServers.length }}
          </span>
        </button>
        <div
          v-if="showMcpPicker"
          class="absolute right-0 mt-1 w-64 bg-white border rounded shadow-lg z-10 p-2 space-y-1"
        >
          <div v-if="assistant.mcpServers.length === 0" class="text-xs text-gray-500 p-1">
            No MCP servers registered.
          </div>
          <label
            v-for="srv in assistant.mcpServers"
            :key="srv.id"
            class="flex items-center gap-2 text-xs p-1 hover:bg-gray-50 rounded cursor-pointer"
            :class="srv.enabled ? '' : 'opacity-50'"
          >
            <input
              type="checkbox"
              :checked="(assistant.current.enabled_mcp_server_ids ?? []).includes(srv.id)"
              :disabled="!srv.enabled"
              @change="toggleMcpServer(srv.id, ($event.target as HTMLInputElement).checked)"
            />
            <span class="flex-1 truncate">{{ srv.name }}</span>
            <span v-if="!srv.enabled" class="text-gray-400">(off)</span>
          </label>
        </div>
      </div>
    </div>
  </header>
</template>