<script setup lang="ts">
// The per-session controls shown in `AssistantView`'s chat header: model,
// agent profile, compact, MCP servers, and generation (temperature/reasoning
// effort) pickers. Split out of `AssistantView.vue` to keep that file under
// the project's line cap — reads `assistant.current` straight from the store
// like the other assistant components (`LiveToolCallList`, etc.) do, rather
// than threading it through props.
import { ref, watch } from 'vue'
import { useAssistantStore } from '../stores/assistant'

const assistant = useAssistantStore()

const emit = defineEmits<{ compacted: [] }>()

async function changeModel(modelId: number) {
  if (!assistant.current) return
  await assistant.updateSession(assistant.current.id, { model_id: modelId })
  await assistant.loadSession(assistant.current.id)
}

async function changeAgentProfile(profile: string) {
  if (!assistant.current) return
  await assistant.updateSession(assistant.current.id, { agent_profile: profile })
  await assistant.loadSession(assistant.current.id)
}

async function compactSession() {
  if (!assistant.current) return
  if (
    !confirm(
      'Compact this chat? The history is summarized and the conversation continues in a fresh session.',
    )
  ) {
    return
  }
  await assistant.compactSession(assistant.current.id)
  emit('compacted')
}

const showMcpPicker = ref(false)

async function toggleMcpServer(serverId: number, on: boolean) {
  if (!assistant.current) return
  const current = assistant.current.enabled_mcp_server_ids ?? []
  const next = on
    ? Array.from(new Set([...current, serverId]))
    : current.filter((id) => id !== serverId)
  await assistant.updateSession(assistant.current.id, {
    enabled_mcp_server_ids: next,
  })
  await assistant.loadSession(assistant.current.id)
}

// Generation controls (temperature / reasoning effort) — kept behind a small
// popover like the MCP picker, since they're less-frequently-changed knobs.
// Draft refs mirror `assistant.current` when the popover opens; only fields
// actually changed to a concrete value get sent (the API has no "clear back
// to null" affordance yet — see PATCH /api/assistant/sessions/{id} note).
const showGenPicker = ref(false)
const tempDraft = ref('')
const reasoningDraft = ref('')
const maxOutputTokensDraft = ref('')
const thinkingBudgetDraft = ref('')

watch(showGenPicker, (open) => {
  if (open && assistant.current) {
    tempDraft.value =
      assistant.current.temperature != null ? String(assistant.current.temperature) : ''
    reasoningDraft.value = assistant.current.reasoning_effort ?? ''
    maxOutputTokensDraft.value =
      assistant.current.max_output_tokens != null
        ? String(assistant.current.max_output_tokens)
        : ''
    thinkingBudgetDraft.value =
      assistant.current.thinking_budget_tokens != null
        ? String(assistant.current.thinking_budget_tokens)
        : ''
  }
})

async function applyTemperature() {
  if (!assistant.current) return
  const trimmed = tempDraft.value.trim()
  if (trimmed === '') return
  const value = Number(trimmed)
  if (Number.isNaN(value) || value === assistant.current.temperature) return
  await assistant.updateSession(assistant.current.id, { temperature: value })
  await assistant.loadSession(assistant.current.id)
}

async function applyReasoningEffort() {
  if (!assistant.current) return
  const effort = reasoningDraft.value
  if (effort === '' || effort === assistant.current.reasoning_effort) return
  await assistant.updateSession(assistant.current.id, { reasoning_effort: effort })
  await assistant.loadSession(assistant.current.id)
}

async function applyMaxOutputTokens() {
  if (!assistant.current) return
  const trimmed = maxOutputTokensDraft.value.trim()
  if (trimmed === '') return
  const value = Number(trimmed)
  if (Number.isNaN(value) || value === assistant.current.max_output_tokens) return
  await assistant.updateSession(assistant.current.id, { max_output_tokens: value })
  await assistant.loadSession(assistant.current.id)
}

async function applyThinkingBudget() {
  if (!assistant.current) return
  const trimmed = thinkingBudgetDraft.value.trim()
  if (trimmed === '') return
  const value = Number(trimmed)
  if (Number.isNaN(value) || value === assistant.current.thinking_budget_tokens) return
  await assistant.updateSession(assistant.current.id, { thinking_budget_tokens: value })
  await assistant.loadSession(assistant.current.id)
}
</script>

<template>
  <div v-if="assistant.current" class="text-xs text-gray-500 flex items-center gap-2">
    <select
      class="border rounded px-2 py-1 text-xs"
      :value="assistant.current.model_id ?? ''"
      @change="changeModel(Number(($event.target as HTMLSelectElement).value))"
    >
      <option v-if="!assistant.current.model_id" :value="''" disabled>
        {{ assistant.current.provider }} / {{ assistant.current.model }}
      </option>
      <option v-for="m in assistant.models" :key="m.id" :value="m.id">
        {{ m.label }} ({{ m.provider_label }})
      </option>
    </select>
    <select
      class="border rounded px-2 py-1 text-xs"
      :value="assistant.current.agent_profile"
      title="Agent profile — what tools this chat may use"
      @change="changeAgentProfile(($event.target as HTMLSelectElement).value)"
    >
      <option value="build">Build</option>
      <option value="researcher">Researcher</option>
      <option value="page-writer">Page writer</option>
    </select>
    <button
      type="button"
      class="border rounded px-2 py-1 text-xs hover:bg-gray-50 disabled:opacity-50"
      title="Summarize this chat's history into a fresh session"
      :disabled="assistant.sending || assistant.current.messages.length === 0"
      @click="compactSession"
    >
      Compact
    </button>
    <div class="relative">
      <button
        type="button"
        class="border rounded px-2 py-1 text-xs hover:bg-gray-50"
        @click="showMcpPicker = !showMcpPicker"
        :title="'MCP servers active in this chat'"
      >
        MCP
        <span class="ml-1 inline-block min-w-[1rem] text-center rounded bg-gray-100 px-1">
          {{ (assistant.current.enabled_mcp_server_ids ?? []).length }}/{{
            assistant.mcpServers.length
          }}
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
    <div class="relative">
      <button
        type="button"
        class="border rounded px-2 py-1 text-xs hover:bg-gray-50"
        @click="showGenPicker = !showGenPicker"
        title="Generation settings (temperature, reasoning effort, max output tokens, thinking budget)"
      >
        Gen
      </button>
      <div
        v-if="showGenPicker"
        class="absolute right-0 mt-1 w-56 bg-white border rounded shadow-lg z-10 p-2 space-y-2"
      >
        <label class="block text-xs">
          <span class="block text-gray-500 mb-1">Temperature</span>
          <input
            type="number"
            step="0.1"
            min="0"
            max="2"
            class="w-full border rounded px-2 py-1 text-xs"
            v-model="tempDraft"
            placeholder="default"
            @blur="applyTemperature"
          />
        </label>
        <label class="block text-xs">
          <span class="block text-gray-500 mb-1">Reasoning effort</span>
          <select
            class="w-full border rounded px-2 py-1 text-xs"
            v-model="reasoningDraft"
            @change="applyReasoningEffort"
          >
            <option value="">(default)</option>
            <option value="low">Low</option>
            <option value="medium">Medium</option>
            <option value="high">High</option>
          </select>
        </label>
        <label class="block text-xs">
          <span class="block text-gray-500 mb-1">Max output tokens</span>
          <input
            type="number"
            step="1"
            min="1"
            class="w-full border rounded px-2 py-1 text-xs"
            v-model="maxOutputTokensDraft"
            placeholder="default"
            @blur="applyMaxOutputTokens"
          />
        </label>
        <label class="block text-xs">
          <span class="block text-gray-500 mb-1">Thinking budget (tokens)</span>
          <input
            type="number"
            step="1"
            min="1"
            class="w-full border rounded px-2 py-1 text-xs"
            v-model="thinkingBudgetDraft"
            placeholder="default"
            @blur="applyThinkingBudget"
          />
        </label>
      </div>
    </div>
  </div>
</template>
