<script setup lang="ts">
import { onMounted } from 'vue'
import { useFilesStore } from '../stores/files'

const props = withDefaults(
  defineProps<{ excludeIds: number[]; mimePrefix?: string }>(),
  { mimePrefix: undefined },
)
const emit = defineEmits<{ pick: [id: number]; close: [] }>()

const files = useFilesStore()
onMounted(() => files.load(props.mimePrefix))
</script>

<template>
  <div class="fixed inset-0 bg-overlay flex items-center justify-center z-50" @click.self="emit('close')">
    <div class="bg-surface rounded-lg shadow-lg max-w-3xl w-full max-h-[80vh] flex flex-col">
      <div class="flex justify-between items-center px-4 py-3 border-b border-line-2">
        <h2 class="font-medium">Pick a file</h2>
        <button class="text-fg-3 hover:text-fg-1" @click="emit('close')">×</button>
      </div>
      <div class="overflow-auto p-3 grid grid-cols-3 sm:grid-cols-4 gap-2">
        <button
          v-for="f in files.items.filter((x) => !excludeIds.includes(x.id))"
          :key="f.id"
          class="text-left bg-surface-alt hover:bg-surface-raised rounded overflow-hidden"
          @click="emit('pick', f.id)"
        >
          <div class="aspect-square bg-surface-muted flex items-center justify-center">
            <img
              v-if="f.has_thumbnail"
              :src="`/files/${f.hash}/nahled`"
              :alt="f.title"
              class="object-cover w-full h-full"
              loading="lazy"
            />
            <span v-else class="text-xs text-fg-4 p-1 text-center break-all">{{ f.mimetype }}</span>
          </div>
          <div class="p-1 text-xs truncate" :title="f.title">{{ f.title }}</div>
        </button>
        <p v-if="files.items.length === 0" class="text-fg-4 col-span-full">No files yet.</p>
      </div>
    </div>
  </div>
</template>
