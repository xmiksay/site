<script setup lang="ts">
import { onMounted } from 'vue'
import { usePagesStore } from '../stores/pages'

const pages = usePagesStore()

onMounted(pages.load)

async function remove(id: number, path: string) {
  if (!confirm(`Delete page "${path}"?`)) return
  await pages.remove(id)
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h1 class="text-xl font-semibold">Pages</h1>
      <router-link
        to="/pages/new"
        class="rounded button-primary px-3 py-1.5 text-sm"
      >
        New page
      </router-link>
    </div>
    <div class="bg-surface rounded-lg shadow overflow-x-auto">
      <table class="min-w-full text-sm">
        <thead class="bg-surface-raised text-fg-2">
          <tr>
            <th class="text-left px-4 py-2">Path</th>
            <th class="text-left px-4 py-2">Summary</th>
            <th class="text-left px-4 py-2">Modified</th>
            <th class="px-4 py-2"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="p in pages.items" :key="p.id" class="border-t border-line-3">
            <td class="px-4 py-2">
              <router-link :to="`/pages/${p.id}/edit`" class="text-accent hover:underline">
                {{ p.path }}
              </router-link>
              <span v-if="p.private" class="ml-2 text-xs bg-surface-muted px-1 rounded">private</span>
            </td>
            <td class="px-4 py-2 text-fg-2">{{ p.summary || '—' }}</td>
            <td class="px-4 py-2 text-fg-3 text-xs">{{ p.modified_at }}</td>
            <td class="px-4 py-2 text-right">
              <button class="text-danger hover:underline" @click="remove(p.id, p.path)">
                Delete
              </button>
            </td>
          </tr>
          <tr v-if="pages.items.length === 0">
            <td colspan="4" class="px-4 py-6 text-center text-fg-4">No pages yet.</td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
