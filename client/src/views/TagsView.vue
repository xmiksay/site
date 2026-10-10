<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useTagsStore } from '../stores/tags'

const tags = useTagsStore()
onMounted(tags.load)

const newName = ref('')
const newDescription = ref('')

async function add() {
  if (!newName.value.trim()) return
  await tags.create({ name: newName.value.trim(), description: newDescription.value || null })
  newName.value = ''
  newDescription.value = ''
}

async function remove(id: number, name: string) {
  if (!confirm(`Delete tag "${name}"?`)) return
  await tags.remove(id)
}

async function rename(id: number) {
  const tag = tags.items.find((t) => t.id === id)
  if (!tag) return
  const newName = prompt('Rename tag', tag.name)
  if (!newName || newName === tag.name) return
  await tags.update(id, { name: newName, description: tag.description })
}
</script>

<template>
  <div class="space-y-4">
    <h1 class="text-xl font-semibold">Tags</h1>
    <form class="bg-surface shadow rounded p-3 flex gap-2 items-end" @submit.prevent="add">
      <label class="flex-1">
        <span class="text-xs text-fg-3">Name</span>
        <input v-model="newName" class="mt-1 w-full rounded border border-line-1 px-2 py-1.5" />
      </label>
      <label class="flex-1">
        <span class="text-xs text-fg-3">Description</span>
        <input v-model="newDescription" class="mt-1 w-full rounded border border-line-1 px-2 py-1.5" />
      </label>
      <button class="rounded button-primary px-3 py-1.5 text-sm">Add</button>
    </form>
    <div class="bg-surface rounded-lg shadow overflow-x-auto">
      <table class="min-w-full text-sm">
        <thead class="bg-surface-raised text-fg-2">
          <tr>
            <th class="text-left px-4 py-2">Name</th>
            <th class="text-left px-4 py-2">Description</th>
            <th class="px-4 py-2"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="t in tags.items" :key="t.id" class="border-t border-line-3">
            <td class="px-4 py-2">{{ t.name }}</td>
            <td class="px-4 py-2 text-fg-2">{{ t.description || '—' }}</td>
            <td class="px-4 py-2 text-right space-x-3">
              <button class="text-accent hover:underline" @click="rename(t.id)">Rename</button>
              <button class="text-danger hover:underline" @click="remove(t.id, t.name)">Delete</button>
            </td>
          </tr>
          <tr v-if="tags.items.length === 0">
            <td colspan="3" class="px-4 py-6 text-center text-fg-4">No tags yet.</td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
