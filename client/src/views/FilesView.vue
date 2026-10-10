<script setup lang="ts">
import { onMounted } from 'vue'
import { useFilesStore } from '../stores/files'
import FileUploader from '../components/FileUploader.vue'
import { formatBytes } from '../lib/format'

const files = useFilesStore()
onMounted(() => files.load())

async function remove(id: number, title: string) {
  if (!confirm(`Delete "${title}"?`)) return
  await files.remove(id)
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h1 class="text-xl font-semibold">Files</h1>
    </div>
    <FileUploader />
    <div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 gap-3">
      <div
        v-for="f in files.items"
        :key="f.id"
        class="bg-surface rounded shadow overflow-hidden"
      >
        <div class="aspect-square bg-surface-raised flex items-center justify-center">
          <img
            v-if="f.has_thumbnail"
            :src="`/files/${f.hash}/nahled`"
            :alt="f.title"
            class="object-cover w-full h-full"
            loading="lazy"
          />
          <div v-else class="text-xs text-fg-4 p-2 text-center break-all">
            {{ f.mimetype }}
          </div>
        </div>
        <div class="p-2 text-sm">
          <div class="truncate font-medium" :title="f.title">{{ f.title }}</div>
          <div class="text-xs text-fg-3 truncate">{{ formatBytes(f.size_bytes) }}</div>
          <div class="mt-2 flex justify-between text-xs">
            <router-link :to="`/files/${f.id}/edit`" class="text-accent hover:underline">
              Edit
            </router-link>
            <button class="text-danger hover:underline" @click="remove(f.id, f.title)">
              Delete
            </button>
          </div>
        </div>
      </div>
      <p v-if="files.items.length === 0" class="text-fg-4 col-span-full">No files yet.</p>
    </div>
  </div>
</template>

<script lang="ts">
export default { name: 'FilesView' }
</script>
