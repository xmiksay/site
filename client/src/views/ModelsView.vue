<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import type { LlmModel, LlmModelInput } from '../types'

const assistant = useAssistantStore()

const showCreate = ref(false)
const draft = ref<LlmModelInput>({
  provider_id: 0,
  label: '',
  model: '',
  is_default: false,
  context_window: undefined,
})

interface EditDraft {
  label: string
  model: string
  context_window: number | undefined
}
const editingId = ref<number | null>(null)
const editDraft = ref<EditDraft>({ label: '', model: '', context_window: undefined })

onMounted(async () => {
  await Promise.all([assistant.loadProviders(), assistant.loadModels()])
  if (!draft.value.provider_id && assistant.providers.length > 0) {
    draft.value.provider_id = assistant.providers[0].id
  }
})

const presets: Record<string, string[]> = {
  anthropic: [
    'claude-opus-4-7',
    'claude-sonnet-4-6',
    'claude-haiku-4-5-20251001',
  ],
  gemini: ['gemini-2.5-pro', 'gemini-2.5-flash', 'gemini-2.5-flash-lite'],
  ollama: ['llama3.1', 'qwen2.5-coder', 'mistral'],
}

function suggestionsFor(providerId: number): string[] {
  const p = assistant.providers.find((p) => p.id === providerId)
  return (p && presets[p.kind]) || []
}

function suggestionsForKind(kind: string): string[] {
  return presets[kind] || []
}

async function create() {
  if (!draft.value.label.trim() || !draft.value.model.trim() || !draft.value.provider_id) return
  await assistant.createModel({
    provider_id: draft.value.provider_id,
    label: draft.value.label.trim(),
    model: draft.value.model.trim(),
    is_default: draft.value.is_default,
    context_window: draft.value.context_window,
  })
  draft.value = {
    provider_id: assistant.providers[0]?.id ?? 0,
    label: '',
    model: '',
    is_default: false,
    context_window: undefined,
  }
  showCreate.value = false
}

function startEdit(m: LlmModel) {
  editingId.value = m.id
  editDraft.value = { label: m.label, model: m.model, context_window: m.context_window ?? undefined }
}

function cancelEdit() {
  editingId.value = null
}

async function saveEdit(m: LlmModel) {
  const patch: Partial<LlmModelInput> = {}
  if (editDraft.value.label.trim() && editDraft.value.label !== m.label) {
    patch.label = editDraft.value.label.trim()
  }
  if (editDraft.value.model.trim() && editDraft.value.model !== m.model) {
    patch.model = editDraft.value.model.trim()
  }
  if (
    editDraft.value.context_window !== undefined &&
    editDraft.value.context_window !== m.context_window
  ) {
    patch.context_window = editDraft.value.context_window
  }
  if (Object.keys(patch).length > 0) {
    await assistant.updateModel(m.id, patch)
  }
  editingId.value = null
}

async function makeDefault(id: number) {
  await assistant.updateModel(id, { is_default: true })
}

async function remove(id: number, label: string) {
  if (!confirm(`Delete model "${label}"?`)) return
  await assistant.deleteModel(id)
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h1 class="text-xl font-semibold">LLM models</h1>
      <button
        class="rounded bg-gray-800 hover:bg-gray-700 text-white px-3 py-1.5 text-sm"
        :disabled="assistant.providers.length === 0"
        @click="showCreate = !showCreate"
      >
        {{ showCreate ? 'Cancel' : 'Add model' }}
      </button>
    </div>

    <p v-if="assistant.providers.length === 0" class="text-sm text-amber-700">
      Add a provider first under
      <router-link to="/providers" class="underline">LLM providers</router-link>.
    </p>

    <div v-if="showCreate" class="bg-white rounded-lg shadow p-4 space-y-3">
      <div class="grid grid-cols-2 gap-3">
        <div>
          <label class="block text-sm font-medium mb-1">Provider</label>
          <select v-model="draft.provider_id" class="w-full border rounded p-2 text-sm">
            <option v-for="p in assistant.providers" :key="p.id" :value="p.id">
              {{ p.label }} ({{ p.kind }})
            </option>
          </select>
        </div>
        <div>
          <label class="block text-sm font-medium mb-1">Label</label>
          <input
            v-model="draft.label"
            class="w-full border rounded p-2 text-sm"
            placeholder="e.g. Sonnet 4.6"
          />
        </div>
      </div>
      <div>
        <label class="block text-sm font-medium mb-1">Model identifier</label>
        <input
          v-model="draft.model"
          class="w-full border rounded p-2 text-sm font-mono"
          list="model-suggestions"
        />
        <datalist id="model-suggestions">
          <option v-for="m in suggestionsFor(draft.provider_id)" :key="m" :value="m" />
        </datalist>
      </div>
      <div>
        <label class="block text-sm font-medium mb-1">Context window (tokens)</label>
        <input
          v-model.number="draft.context_window"
          type="number"
          min="1"
          class="w-full border rounded p-2 text-sm"
          placeholder="e.g. 200000 — leave blank for the engine default"
        />
      </div>
      <label class="flex items-center gap-2 text-sm">
        <input v-model="draft.is_default" type="checkbox" /> default for new chats
      </label>
      <div class="flex justify-end">
        <button class="rounded bg-gray-800 text-white px-4 py-2 text-sm" @click="create">Save</button>
      </div>
    </div>

    <div class="bg-white rounded-lg shadow overflow-x-auto">
      <table class="min-w-full text-sm">
        <thead class="bg-gray-100 text-gray-600">
          <tr>
            <th class="text-left px-4 py-2">Label</th>
            <th class="text-left px-4 py-2">Provider</th>
            <th class="text-left px-4 py-2">Model</th>
            <th class="text-left px-4 py-2">Context window</th>
            <th class="text-left px-4 py-2">Default</th>
            <th class="px-4 py-2"></th>
          </tr>
        </thead>
        <tbody>
          <template v-for="m in assistant.models" :key="m.id">
            <tr class="border-t border-gray-100">
              <td class="px-4 py-2 font-medium">{{ m.label }}</td>
              <td class="px-4 py-2 text-gray-600">{{ m.provider_label }} ({{ m.provider_kind }})</td>
              <td class="px-4 py-2 font-mono text-xs">{{ m.model }}</td>
              <td class="px-4 py-2 text-gray-600">{{ m.context_window ?? '—' }}</td>
              <td class="px-4 py-2">
                <span v-if="m.is_default" class="text-xs text-emerald-700 font-semibold">default</span>
                <button
                  v-else
                  class="text-xs text-blue-600 hover:underline"
                  @click="makeDefault(m.id)"
                >
                  make default
                </button>
              </td>
              <td class="px-4 py-2 text-right space-x-3">
                <button
                  v-if="editingId !== m.id"
                  class="text-xs text-blue-600 hover:underline"
                  @click="startEdit(m)"
                >
                  edit
                </button>
                <button
                  v-else
                  class="text-xs text-gray-600 hover:underline"
                  @click="cancelEdit"
                >
                  cancel
                </button>
                <button class="text-xs text-red-500 hover:underline" @click="remove(m.id, m.label)">
                  delete
                </button>
              </td>
            </tr>
            <tr v-if="editingId === m.id" class="border-t border-gray-100 bg-gray-50">
              <td colspan="6" class="px-4 py-3">
                <div class="space-y-3">
                  <div class="grid grid-cols-2 gap-3">
                    <div>
                      <label class="block text-xs font-medium mb-1">Label</label>
                      <input v-model="editDraft.label" class="w-full border rounded p-2 text-sm" />
                    </div>
                    <div>
                      <label class="block text-xs font-medium mb-1">Model identifier</label>
                      <input
                        v-model="editDraft.model"
                        class="w-full border rounded p-2 text-sm font-mono"
                        :list="`model-suggestions-edit-${m.id}`"
                      />
                      <datalist :id="`model-suggestions-edit-${m.id}`">
                        <option
                          v-for="opt in suggestionsForKind(m.provider_kind)"
                          :key="opt"
                          :value="opt"
                        />
                      </datalist>
                    </div>
                  </div>
                  <div>
                    <label class="block text-xs font-medium mb-1">Context window (tokens)</label>
                    <input
                      v-model.number="editDraft.context_window"
                      type="number"
                      min="1"
                      class="w-full border rounded p-2 text-sm"
                    />
                  </div>
                  <div class="flex justify-end">
                    <button
                      class="rounded bg-gray-800 text-white px-4 py-2 text-sm"
                      @click="saveEdit(m)"
                    >
                      Save changes
                    </button>
                  </div>
                </div>
              </td>
            </tr>
          </template>
          <tr v-if="assistant.models.length === 0">
            <td colspan="6" class="px-4 py-6 text-center text-gray-400">
              No models yet.
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
