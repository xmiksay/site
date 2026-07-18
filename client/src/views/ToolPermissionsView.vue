<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import type { ToolPermissionInput } from '../types'

const assistant = useAssistantStore()

const showCreate = ref(false)
const draft = ref<ToolPermissionInput>({
  name: '',
  effect: 'allow',
  priority: 100,
})

onMounted(assistant.loadPermissions)

async function create() {
  if (!draft.value.name.trim()) return
  await assistant.createPermission({
    name: draft.value.name.trim(),
    effect: draft.value.effect,
    priority: draft.value.priority,
  })
  draft.value = { name: '', effect: 'allow', priority: 100 }
  showCreate.value = false
}

async function setEffect(id: number, effect: string) {
  await assistant.updatePermission(id, { effect })
}

async function setPriority(id: number, priority: number) {
  await assistant.updatePermission(id, { priority })
}

async function remove(id: number, name: string) {
  if (!confirm(`Delete rule for "${name}"?`)) return
  await assistant.deletePermission(id)
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h1 class="text-xl font-semibold">Tool permissions</h1>
      <button
        class="rounded bg-gray-800 hover:bg-gray-700 text-white px-3 py-1.5 text-sm"
        @click="showCreate = !showCreate"
      >
        {{ showCreate ? 'Cancel' : 'Add rule' }}
      </button>
    </div>

    <p class="text-sm text-gray-600">
      The assistant runs every tool call against these rules in priority order (lower runs first).
      Default for unmatched calls is <code>prompt</code> — you'll see approve / reject buttons in
      the chat. A rule name is a literal tool name (e.g. <code>edit_page</code>), the catch-all
      <code>*</code>, a capability key <code>read</code> / <code>write</code> / <code>call</code>
      (grades every tool of that kind at once, including annotated MCP tools — see MCP servers'
      capability hints), or a scoped form: <code>tool(pattern)</code> matches a tool's scoping
      argument (e.g. <code>edit_page(obsidian/*)</code>, <code>web_search(rust *)</code>) and
      <code>tool{pattern}</code> matches its working directory (reserved — no built-in tool
      exposes one yet). A capability key accepts scoping too, e.g. <code>write(obsidian/*)</code>.
    </p>

    <div v-if="showCreate" class="bg-white rounded-lg shadow p-4 space-y-3">
      <div class="grid grid-cols-3 gap-3">
        <div class="col-span-2">
          <label class="block text-sm font-medium mb-1">Tool name</label>
          <input
            v-model="draft.name"
            class="w-full border rounded p-2 text-sm font-mono"
            placeholder="e.g. read_page, read, or edit_page(obsidian/*)"
          />
        </div>
        <div>
          <label class="block text-sm font-medium mb-1">Effect</label>
          <select v-model="draft.effect" class="w-full border rounded p-2 text-sm">
            <option value="allow">allow</option>
            <option value="deny">deny</option>
            <option value="prompt">prompt</option>
          </select>
        </div>
      </div>
      <div>
        <label class="block text-sm font-medium mb-1">Priority</label>
        <input
          v-model.number="draft.priority"
          type="number"
          class="w-32 border rounded p-2 text-sm"
        />
      </div>
      <div class="flex justify-end">
        <button class="rounded bg-gray-800 text-white px-4 py-2 text-sm" @click="create">Save</button>
      </div>
    </div>

    <div class="bg-white rounded-lg shadow overflow-x-auto">
      <table class="min-w-full text-sm">
        <thead class="bg-gray-100 text-gray-600">
          <tr>
            <th class="text-left px-4 py-2">Priority</th>
            <th class="text-left px-4 py-2">Tool name</th>
            <th class="text-left px-4 py-2">Effect</th>
            <th class="px-4 py-2"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="r in assistant.permissions" :key="r.id" class="border-t border-gray-100">
            <td class="px-4 py-2">
              <input
                type="number"
                :value="r.priority"
                @change="setPriority(r.id, Number(($event.target as HTMLInputElement).value))"
                class="w-16 border rounded px-2 py-1 text-xs"
              />
            </td>
            <td class="px-4 py-2 font-mono">{{ r.name }}</td>
            <td class="px-4 py-2">
              <select
                :value="r.effect"
                @change="setEffect(r.id, ($event.target as HTMLSelectElement).value)"
                class="border rounded px-2 py-1 text-xs"
              >
                <option value="allow">allow</option>
                <option value="deny">deny</option>
                <option value="prompt">prompt</option>
              </select>
            </td>
            <td class="px-4 py-2 text-right">
              <button class="text-xs text-red-500 hover:underline" @click="remove(r.id, r.name)">
                delete
              </button>
            </td>
          </tr>
          <tr v-if="assistant.permissions.length === 0">
            <td colspan="4" class="px-4 py-6 text-center text-gray-400">
              No rules — every tool call requires explicit approval.
            </td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
