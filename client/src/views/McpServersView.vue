<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import type { McpServer } from '../types'

const assistant = useAssistantStore()

const showCreate = ref(false)
const draft = ref({
  name: '',
  url: '',
  forward_user_token: false,
  enabled: true,
  headersText: '',
  capabilitiesText: '',
})

interface EditDraft {
  name: string
  url: string
  forward_user_token: boolean
  enabled: boolean
  headersText: string
  capabilitiesText: string
}
const editingId = ref<number | null>(null)
const editDraft = ref<EditDraft>({
  name: '',
  url: '',
  forward_user_token: false,
  enabled: true,
  headersText: '',
  capabilitiesText: '',
})

onMounted(assistant.loadMcpServers)

function parseColonLines(raw: string): Record<string, string> {
  const out: Record<string, string> = {}
  raw
    .split('\n')
    .map((s) => s.trim())
    .filter(Boolean)
    .forEach((line) => {
      const idx = line.indexOf(':')
      if (idx > 0) {
        out[line.slice(0, idx).trim()] = line.slice(idx + 1).trim()
      }
    })
  return out
}

function mapToText(map: Record<string, string>): string {
  return Object.entries(map)
    .map(([k, v]) => `${k}: ${v}`)
    .join('\n')
}

async function create() {
  if (!draft.value.name || !draft.value.url) return
  await assistant.createMcpServer({
    name: draft.value.name,
    url: draft.value.url,
    enabled: draft.value.enabled,
    forward_user_token: draft.value.forward_user_token,
    headers: parseColonLines(draft.value.headersText),
    capabilities: parseColonLines(draft.value.capabilitiesText),
  })
  draft.value = {
    name: '',
    url: '',
    forward_user_token: false,
    enabled: true,
    headersText: '',
    capabilitiesText: '',
  }
  showCreate.value = false
  await assistant.loadMcpServers()
}

function startEdit(s: McpServer) {
  editingId.value = s.id
  editDraft.value = {
    name: s.name,
    url: s.url,
    forward_user_token: s.forward_user_token,
    enabled: s.enabled,
    headersText: mapToText(s.headers || {}),
    capabilitiesText: mapToText(s.capabilities || {}),
  }
}

function cancelEdit() {
  editingId.value = null
}

async function saveEdit(id: number) {
  if (!editDraft.value.name.trim() || !editDraft.value.url.trim()) return
  await assistant.updateMcpServer(id, {
    name: editDraft.value.name.trim(),
    url: editDraft.value.url.trim(),
    enabled: editDraft.value.enabled,
    forward_user_token: editDraft.value.forward_user_token,
    headers: parseColonLines(editDraft.value.headersText),
    capabilities: parseColonLines(editDraft.value.capabilitiesText),
  })
  editingId.value = null
  await assistant.loadMcpServers()
}

async function toggle(id: number, enabled: boolean) {
  await assistant.updateMcpServer(id, { enabled })
  await assistant.loadMcpServers()
}

async function remove(id: number, name: string) {
  if (!confirm(`Delete MCP server "${name}"?`)) return
  await assistant.deleteMcpServer(id)
  await assistant.loadMcpServers()
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h1 class="text-xl font-semibold">MCP servers</h1>
      <button
        class="rounded bg-gray-800 hover:bg-gray-700 text-white px-3 py-1.5 text-sm"
        @click="showCreate = !showCreate"
      >
        {{ showCreate ? 'Cancel' : 'Add server' }}
      </button>
    </div>

    <div v-if="showCreate" class="bg-white rounded-lg shadow p-4 space-y-3">
      <div>
        <label class="block text-sm font-medium mb-1">Name</label>
        <input
          v-model="draft.name"
          class="w-full border rounded p-2 text-sm"
          placeholder="e.g. github"
        />
      </div>
      <div>
        <label class="block text-sm font-medium mb-1">URL</label>
        <input
          v-model="draft.url"
          class="w-full border rounded p-2 text-sm"
          placeholder="https://example.com/mcp"
        />
      </div>
      <div>
        <label class="block text-sm font-medium mb-1">Custom headers</label>
        <textarea
          v-model="draft.headersText"
          rows="3"
          class="w-full border rounded p-2 text-sm font-mono"
          placeholder="Authorization: Bearer xyz&#10;X-Custom: value"
        ></textarea>
        <p class="text-xs text-gray-500 mt-1">One header per line, in <code>Name: value</code> form.</p>
      </div>
      <div>
        <label class="block text-sm font-medium mb-1">Capabilities</label>
        <textarea
          v-model="draft.capabilitiesText"
          rows="3"
          class="w-full border rounded p-2 text-sm font-mono"
          placeholder="search: read&#10;delete_item: write"
        ></textarea>
        <p class="text-xs text-gray-500 mt-1">
          One remote tool per line, in <code>tool_name: read|write|call</code> form — lets a
          <code>tool_permissions</code> capability rule fan out to this server's tools.
        </p>
      </div>
      <div class="flex items-center gap-4">
        <label class="text-sm">
          <input v-model="draft.enabled" type="checkbox" /> enabled
        </label>
        <label class="text-sm">
          <input v-model="draft.forward_user_token" type="checkbox" /> forward my session as bearer
        </label>
      </div>
      <div class="flex justify-end">
        <button
          class="rounded bg-gray-800 text-white px-4 py-2 text-sm"
          @click="create"
        >
          Save
        </button>
      </div>
    </div>

    <div class="bg-white rounded-lg shadow overflow-x-auto">
      <table class="min-w-full text-sm">
        <thead class="bg-gray-100 text-gray-600">
          <tr>
            <th class="text-left px-4 py-2">Name</th>
            <th class="text-left px-4 py-2">URL</th>
            <th class="text-left px-4 py-2">Enabled</th>
            <th class="text-left px-4 py-2">Forward token</th>
            <th class="px-4 py-2"></th>
          </tr>
        </thead>
        <tbody>
          <template v-for="s in assistant.mcpServers" :key="s.id">
            <tr class="border-t border-gray-100">
              <td class="px-4 py-2 font-medium">{{ s.name }}</td>
              <td class="px-4 py-2 truncate max-w-md text-gray-600">{{ s.url }}</td>
              <td class="px-4 py-2">
                <input
                  type="checkbox"
                  :checked="s.enabled"
                  @change="toggle(s.id, ($event.target as HTMLInputElement).checked)"
                />
              </td>
              <td class="px-4 py-2 text-gray-600">
                {{ s.forward_user_token ? 'yes' : 'no' }}
              </td>
              <td class="px-4 py-2 text-right space-x-3">
                <button
                  v-if="editingId !== s.id"
                  class="text-blue-600 hover:underline text-xs"
                  @click="startEdit(s)"
                >
                  edit
                </button>
                <button
                  v-else
                  class="text-gray-600 hover:underline text-xs"
                  @click="cancelEdit"
                >
                  cancel
                </button>
                <button
                  class="text-red-500 hover:underline text-xs"
                  @click="remove(s.id, s.name)"
                >
                  delete
                </button>
              </td>
            </tr>
            <tr v-if="editingId === s.id" class="border-t border-gray-100 bg-gray-50">
              <td colspan="5" class="px-4 py-3">
                <div class="space-y-3">
                  <div class="grid grid-cols-2 gap-3">
                    <div>
                      <label class="block text-xs font-medium mb-1">Name</label>
                      <input v-model="editDraft.name" class="w-full border rounded p-2 text-sm" />
                    </div>
                    <div>
                      <label class="block text-xs font-medium mb-1">URL</label>
                      <input v-model="editDraft.url" class="w-full border rounded p-2 text-sm" />
                    </div>
                  </div>
                  <div>
                    <label class="block text-xs font-medium mb-1">Custom headers</label>
                    <textarea
                      v-model="editDraft.headersText"
                      rows="3"
                      class="w-full border rounded p-2 text-sm font-mono"
                      placeholder="Authorization: Bearer xyz"
                    ></textarea>
                    <p class="text-xs text-gray-500 mt-1">One header per line, in <code>Name: value</code> form.</p>
                  </div>
                  <div>
                    <label class="block text-xs font-medium mb-1">Capabilities</label>
                    <textarea
                      v-model="editDraft.capabilitiesText"
                      rows="3"
                      class="w-full border rounded p-2 text-sm font-mono"
                      placeholder="search: read"
                    ></textarea>
                    <p class="text-xs text-gray-500 mt-1">
                      One remote tool per line, in <code>tool_name: read|write|call</code> form.
                    </p>
                  </div>
                  <div class="flex items-center gap-4">
                    <label class="text-sm">
                      <input v-model="editDraft.enabled" type="checkbox" /> enabled
                    </label>
                    <label class="text-sm">
                      <input v-model="editDraft.forward_user_token" type="checkbox" /> forward my session as bearer
                    </label>
                  </div>
                  <div class="flex justify-end">
                    <button
                      class="rounded bg-gray-800 text-white px-4 py-2 text-sm"
                      @click="saveEdit(s.id)"
                    >
                      Save changes
                    </button>
                  </div>
                </div>
              </td>
            </tr>
          </template>
          <tr v-if="assistant.mcpServers.length === 0">
            <td colspan="5" class="px-4 py-6 text-center text-gray-400">
              No MCP servers registered.
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <div v-if="assistant.discovered.length > 0" class="space-y-3">
      <h2 class="font-semibold">Discovered tools</h2>
      <div
        v-for="server in assistant.discovered"
        :key="server.name"
        class="bg-white rounded-lg shadow p-4"
      >
        <div class="flex items-center justify-between mb-2">
          <div>
            <span class="font-medium">{{ server.name }}</span>
            <span class="text-xs text-gray-500 ml-2">{{ server.url }}</span>
          </div>
          <span
            class="text-xs px-2 py-0.5 rounded"
            :class="server.connected ? 'bg-green-100 text-green-800' : 'bg-red-100 text-red-700'"
          >
            {{ server.connected ? 'connected' : 'unreachable' }}
          </span>
        </div>
        <ul class="text-sm space-y-1">
          <li v-for="t in server.tools" :key="t.prefixed_name" class="flex gap-2">
            <code class="text-gray-700 shrink-0">{{ t.prefixed_name }}</code>
            <span class="text-gray-500 truncate">{{ t.description }}</span>
          </li>
          <li v-if="server.tools.length === 0" class="text-gray-400 italic">
            no tools discovered
          </li>
        </ul>
      </div>
    </div>
  </div>
</template>
