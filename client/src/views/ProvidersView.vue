<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import type { LlmProvider, LlmProviderInput } from '../types'

const assistant = useAssistantStore()

const showCreate = ref(false)
const draft = ref<LlmProviderInput>({
  label: '',
  kind: 'anthropic',
  api_key: '',
  base_url: '',
  concurrency: undefined,
  rpm: undefined,
})

interface EditDraft {
  label: string
  api_key: string
  api_key_dirty: boolean
  base_url: string
  concurrency?: number
  concurrency_dirty: boolean
  rpm?: number
  rpm_dirty: boolean
}
const editingId = ref<number | null>(null)
const editDraft = ref<EditDraft>({
  label: '',
  api_key: '',
  api_key_dirty: false,
  base_url: '',
  concurrency: undefined,
  concurrency_dirty: false,
  rpm: undefined,
  rpm_dirty: false,
})

/** Round to a whole number, or `undefined` for anything non-positive (the
 * "not set" sentinel `create`/`saveEdit` map to omit/clear). Guards against a
 * `type="number"` input accepting a decimal that would 422 as an invalid i32
 * on the wire instead of surfacing `validate_budget`'s friendly message. */
function wholePositive(n: number | undefined): number | undefined {
  if (n === undefined || !Number.isFinite(n) || n <= 0) return undefined
  return Math.round(n)
}

onMounted(assistant.loadProviders)

const presets: Record<string, { needsKey: boolean; needsUrl: boolean; defaultUrl?: string }> = {
  anthropic: { needsKey: true, needsUrl: false },
  gemini: { needsKey: true, needsUrl: false },
  ollama: { needsKey: false, needsUrl: true, defaultUrl: 'http://localhost:11434' },
  // `needsKey: false` here means "optional", not "must be empty" — unlike ollama, an
  // openai-kind endpoint may still require a key (z.ai, hosted OpenAI, ...), so
  // `onKindChange` must not auto-clear it the way it does for ollama.
  openai: { needsKey: false, needsUrl: true },
}

// Quick-fill options for the openai kind's base_url — it covers several vendors,
// so there's no single `presets.defaultUrl` the way ollama has.
const openaiPresets = [
  { label: 'z.ai (Coding Plan)', url: 'https://api.z.ai/api/coding/paas/v4' },
  { label: 'z.ai (General / pay-as-you-go)', url: 'https://api.z.ai/api/paas/v4' },
  { label: 'OpenAI', url: 'https://api.openai.com/v1' },
  { label: 'Custom', url: 'custom' },
]

function applyOpenaiPreset(url: string) {
  draft.value.base_url = url === 'custom' ? '' : url
}

function onKindChange() {
  const p = presets[draft.value.kind]
  if (!p) return
  // Only ollama is structurally keyless; openai's `needsKey: false` just means
  // "not required", so switching into it must not wipe a key the user already typed.
  if (draft.value.kind === 'ollama') draft.value.api_key = ''
  if (!p.needsUrl) draft.value.base_url = ''
  if (p.needsUrl && !draft.value.base_url) draft.value.base_url = p.defaultUrl ?? ''
}

async function create() {
  if (!draft.value.label.trim()) return
  await assistant.createProvider({
    label: draft.value.label.trim(),
    kind: draft.value.kind,
    api_key: draft.value.api_key?.trim() || undefined,
    base_url: draft.value.base_url?.trim() || undefined,
    concurrency: wholePositive(draft.value.concurrency ?? undefined),
    rpm: wholePositive(draft.value.rpm ?? undefined),
  })
  draft.value = {
    label: '',
    kind: 'anthropic',
    api_key: '',
    base_url: '',
    concurrency: undefined,
    rpm: undefined,
  }
  showCreate.value = false
}

function startEdit(p: LlmProvider) {
  editingId.value = p.id
  editDraft.value = {
    label: p.label,
    api_key: '',
    api_key_dirty: false,
    base_url: p.base_url ?? '',
    concurrency: p.concurrency ?? undefined,
    concurrency_dirty: false,
    rpm: p.rpm ?? undefined,
    rpm_dirty: false,
  }
}

function cancelEdit() {
  editingId.value = null
}

async function saveEdit(p: LlmProvider) {
  if (!editDraft.value.label.trim()) return
  const patch: Partial<LlmProviderInput> = { label: editDraft.value.label.trim() }
  // openai carries both a base_url and an optional api_key, so unlike the old
  // ollama-vs-everyone-else split these can't be mutually exclusive branches.
  if (p.kind === 'ollama' || p.kind === 'openai') {
    patch.base_url = editDraft.value.base_url.trim()
  }
  if (editDraft.value.api_key_dirty) {
    patch.api_key = editDraft.value.api_key
  }
  // Only touched fields ride the patch; a blank/zeroed input while dirty
  // sends `null` to clear it back to the engine default (see
  // `LlmProviderInput.concurrency`'s doc and the backend's double-`Option`
  // `UpdateProvider` handling) rather than being silently dropped.
  if (editDraft.value.concurrency_dirty) {
    patch.concurrency = wholePositive(editDraft.value.concurrency) ?? null
  }
  if (editDraft.value.rpm_dirty) {
    patch.rpm = wholePositive(editDraft.value.rpm) ?? null
  }
  await assistant.updateProvider(p.id, patch)
  editingId.value = null
}

async function remove(id: number, label: string) {
  if (!confirm(`Delete provider "${label}"? This will also drop its models.`)) return
  await assistant.deleteProvider(id)
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h1 class="text-xl font-semibold">LLM providers</h1>
      <button
        class="rounded bg-gray-800 hover:bg-gray-700 text-white px-3 py-1.5 text-sm"
        @click="showCreate = !showCreate"
      >
        {{ showCreate ? 'Cancel' : 'Add provider' }}
      </button>
    </div>

    <p class="text-sm text-gray-600">
      A provider is just the connection (API key or local URL). Add models for it under
      <router-link to="/models" class="text-blue-600 hover:underline">LLM models</router-link>.
    </p>

    <div v-if="showCreate" class="bg-white rounded-lg shadow p-4 space-y-3">
      <div class="grid grid-cols-2 gap-3">
        <div>
          <label class="block text-sm font-medium mb-1">Label</label>
          <input v-model="draft.label" class="w-full border rounded p-2 text-sm" placeholder="e.g. Anthropic" />
        </div>
        <div>
          <label class="block text-sm font-medium mb-1">Kind</label>
          <select
            v-model="draft.kind"
            @change="onKindChange"
            class="w-full border rounded p-2 text-sm"
          >
            <option value="anthropic">Anthropic (Claude)</option>
            <option value="gemini">Google Gemini</option>
            <option value="ollama">Ollama (local)</option>
            <option value="openai">OpenAI-compatible (incl. z.ai)</option>
          </select>
        </div>
      </div>
      <div v-if="draft.kind === 'openai'">
        <label class="block text-sm font-medium mb-1">Preset</label>
        <select
          class="w-full border rounded p-2 text-sm"
          @change="applyOpenaiPreset(($event.target as HTMLSelectElement).value)"
        >
          <option value="">Choose a preset…</option>
          <option v-for="opt in openaiPresets" :key="opt.url" :value="opt.url">{{ opt.label }}</option>
        </select>
      </div>
      <div v-if="draft.kind !== 'ollama'">
        <label class="block text-sm font-medium mb-1">
          API key<span v-if="!presets[draft.kind]?.needsKey"> (optional)</span>
        </label>
        <input v-model="draft.api_key" type="password" class="w-full border rounded p-2 text-sm font-mono" placeholder="sk-..." />
      </div>
      <div v-if="draft.kind === 'ollama' || draft.kind === 'openai'">
        <label class="block text-sm font-medium mb-1">Base URL</label>
        <input v-model="draft.base_url" class="w-full border rounded p-2 text-sm font-mono" placeholder="http://localhost:11434" />
      </div>
      <div class="grid grid-cols-2 gap-3">
        <div>
          <label class="block text-sm font-medium mb-1">Max concurrency</label>
          <input
            v-model.number="draft.concurrency"
            type="number"
            min="1"
            step="1"
            class="w-full border rounded p-2 text-sm"
            placeholder="default (3)"
          />
        </div>
        <div>
          <label class="block text-sm font-medium mb-1">Requests/min</label>
          <input
            v-model.number="draft.rpm"
            type="number"
            min="1"
            step="1"
            class="w-full border rounded p-2 text-sm"
            placeholder="default (50)"
          />
        </div>
      </div>
      <div class="flex justify-end">
        <button class="rounded bg-gray-800 text-white px-4 py-2 text-sm" @click="create">Save</button>
      </div>
    </div>

    <div class="bg-white rounded-lg shadow overflow-x-auto">
      <table class="min-w-full text-sm">
        <thead class="bg-gray-100 text-gray-600">
          <tr>
            <th class="text-left px-4 py-2">Label</th>
            <th class="text-left px-4 py-2">Kind</th>
            <th class="text-left px-4 py-2">Configured</th>
            <th class="text-left px-4 py-2">Limits</th>
            <th class="px-4 py-2"></th>
          </tr>
        </thead>
        <tbody>
          <template v-for="p in assistant.providers" :key="p.id">
            <tr class="border-t border-gray-100">
              <td class="px-4 py-2 font-medium">{{ p.label }}</td>
              <td class="px-4 py-2 text-gray-600">{{ p.kind }}</td>
              <td class="px-4 py-2 text-gray-600">
                <span v-if="p.kind === 'ollama'">{{ p.base_url || '—' }}</span>
                <span v-else-if="p.kind === 'openai'">
                  {{ p.base_url || '—' }} ·
                  <span v-if="p.has_api_key">key set</span>
                  <span v-else class="text-amber-600">no key</span>
                </span>
                <span v-else-if="p.has_api_key">key set</span>
                <span v-else class="text-red-600">no api key</span>
              </td>
              <td class="px-4 py-2 text-gray-600">
                <span v-if="p.concurrency == null && p.rpm == null" class="text-gray-400">default</span>
                <span v-else>{{ p.concurrency ?? 'default' }} conc / {{ p.rpm ?? 'default' }} rpm</span>
              </td>
              <td class="px-4 py-2 text-right space-x-3">
                <button
                  v-if="editingId !== p.id"
                  class="text-xs text-blue-600 hover:underline"
                  @click="startEdit(p)"
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
                <button class="text-xs text-red-500 hover:underline" @click="remove(p.id, p.label)">
                  delete
                </button>
              </td>
            </tr>
            <tr v-if="editingId === p.id" class="border-t border-gray-100 bg-gray-50">
              <td colspan="5" class="px-4 py-3">
                <div class="space-y-3">
                  <div>
                    <label class="block text-xs font-medium mb-1">Label</label>
                    <input v-model="editDraft.label" class="w-full border rounded p-2 text-sm" />
                  </div>
                  <div v-if="p.kind === 'ollama' || p.kind === 'openai'">
                    <label class="block text-xs font-medium mb-1">Base URL</label>
                    <input v-model="editDraft.base_url" class="w-full border rounded p-2 text-sm font-mono" />
                  </div>
                  <div v-if="p.kind !== 'ollama'">
                    <label class="block text-xs font-medium mb-1">API key</label>
                    <input
                      v-model="editDraft.api_key"
                      type="password"
                      class="w-full border rounded p-2 text-sm font-mono"
                      :placeholder="p.has_api_key ? 'leave blank to keep current key' : 'sk-...'"
                      @input="editDraft.api_key_dirty = true"
                    />
                    <p class="text-xs text-gray-500 mt-1">
                      Submit empty to clear the stored key.
                    </p>
                  </div>
                  <div class="grid grid-cols-2 gap-3">
                    <div>
                      <label class="block text-xs font-medium mb-1">Max concurrency</label>
                      <input
                        v-model.number="editDraft.concurrency"
                        type="number"
                        min="1"
                        step="1"
                        class="w-full border rounded p-2 text-sm"
                        placeholder="default (3)"
                        @input="editDraft.concurrency_dirty = true"
                      />
                    </div>
                    <div>
                      <label class="block text-xs font-medium mb-1">Requests/min</label>
                      <input
                        v-model.number="editDraft.rpm"
                        type="number"
                        min="1"
                        step="1"
                        class="w-full border rounded p-2 text-sm"
                        placeholder="default (50)"
                        @input="editDraft.rpm_dirty = true"
                      />
                    </div>
                  </div>
                  <p class="text-xs text-gray-500">
                    Clear a limit and save to reset it back to the default.
                  </p>
                  <div class="flex justify-end">
                    <button
                      class="rounded bg-gray-800 text-white px-4 py-2 text-sm"
                      @click="saveEdit(p)"
                    >
                      Save changes
                    </button>
                  </div>
                </div>
              </td>
            </tr>
          </template>
          <tr v-if="assistant.providers.length === 0">
            <td colspan="5" class="px-4 py-6 text-center text-gray-400">
              No providers yet. Add one to start.
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
