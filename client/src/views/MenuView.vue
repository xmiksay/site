<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { useMenuStore } from '../stores/menu'
import PathPicker from '../components/PathPicker.vue'
import MarkdownEditor from '../components/MarkdownEditor.vue'
import type { MenuItem } from '../types'

const props = defineProps<{ id?: string }>()

const menu = useMenuStore()
onMounted(async () => {
  await menu.load()
  if (props.id) {
    const target = menu.items.find((m) => m.id === Number(props.id))
    if (target) startEdit(target)
  }
})

const editing = ref<number | null>(null)
const draft = reactive<{
  title: string
  path: string
  markdown: string
  order_index: number
  private: boolean
}>({ title: '', path: '', markdown: '', order_index: 0, private: false })

function startNew() {
  editing.value = -1
  draft.title = ''
  draft.path = ''
  draft.markdown = ''
  draft.order_index = (menu.items.at(-1)?.order_index ?? 0) + 1
  draft.private = false
}

function startEdit(item: MenuItem) {
  editing.value = item.id
  draft.title = item.title
  draft.path = item.path
  draft.markdown = item.markdown
  draft.order_index = item.order_index
  draft.private = item.private
}

async function save() {
  const input = { ...draft }
  if (editing.value === -1) {
    await menu.create(input)
  } else if (editing.value !== null) {
    await menu.update(editing.value, input)
  }
  editing.value = null
}

async function remove(item: MenuItem) {
  if (!confirm(`Delete menu item "${item.title}"?`)) return
  await menu.remove(item.id)
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h1 class="text-xl font-semibold">Menu</h1>
      <button
        class="rounded button-primary px-3 py-1.5 text-sm"
        @click="startNew"
      >
        New entry
      </button>
    </div>
    <div class="bg-surface rounded-lg shadow overflow-x-auto">
      <table class="min-w-full text-sm">
        <thead class="bg-surface-raised text-fg-2">
          <tr>
            <th class="text-left px-4 py-2">#</th>
            <th class="text-left px-4 py-2">Title</th>
            <th class="text-left px-4 py-2">Path</th>
            <th class="px-4 py-2"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="m in menu.items" :key="m.id" class="border-t border-line-3">
            <td class="px-4 py-2 text-fg-3">{{ m.order_index }}</td>
            <td class="px-4 py-2">
              {{ m.title }}
              <span v-if="m.private" class="ml-1 text-xs bg-surface-muted px-1 rounded">private</span>
            </td>
            <td class="px-4 py-2 text-fg-2">{{ m.path }}</td>
            <td class="px-4 py-2 text-right space-x-3">
              <button class="text-accent hover:underline" @click="startEdit(m)">Edit</button>
              <button class="text-danger hover:underline" @click="remove(m)">Delete</button>
            </td>
          </tr>
          <tr v-if="menu.items.length === 0">
            <td colspan="4" class="px-4 py-6 text-center text-fg-4">No menu entries.</td>
          </tr>
        </tbody>
      </table>
    </div>

    <div v-if="editing !== null" class="bg-surface shadow rounded p-4 space-y-3 max-w-2xl">
      <h2 class="font-medium">{{ editing === -1 ? 'New entry' : 'Edit entry' }}</h2>
      <div class="grid grid-cols-2 gap-3">
        <label class="block">
          <span class="text-sm text-fg-2">Title</span>
          <input v-model="draft.title" class="mt-1 w-full rounded border border-line-1 px-2 py-1.5" />
        </label>
        <label class="block">
          <span class="text-sm text-fg-2">Path</span>
          <PathPicker v-model="draft.path" namespace="all" class="mt-1" />
        </label>
        <label class="block">
          <span class="text-sm text-fg-2">Order index</span>
          <input
            v-model.number="draft.order_index"
            type="number"
            class="mt-1 w-full rounded border border-line-1 px-2 py-1.5"
          />
        </label>
        <label class="inline-flex items-center gap-2 mt-6 text-sm">
          <input v-model="draft.private" type="checkbox" />
          Private
        </label>
      </div>
      <label class="block">
        <span class="text-sm text-fg-2">Markdown</span>
        <MarkdownEditor v-model="draft.markdown" :rows="6" class="mt-1" />
      </label>
      <div class="space-x-2 text-sm">
        <button class="text-fg-2 hover:underline" @click="editing = null">Cancel</button>
        <button
          class="rounded button-primary px-3 py-1.5"
          @click="save"
        >
          Save
        </button>
      </div>
    </div>
  </div>
</template>
