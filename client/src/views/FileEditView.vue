<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useFilesStore } from '../stores/files'
import PathPicker from '../components/PathPicker.vue'
import type { FileSummary } from '../types'

const props = defineProps<{ id: string }>()

const router = useRouter()
const files = useFilesStore()

const meta = ref<FileSummary | null>(null)
const path = ref('')
const description = ref('')
const error = ref('')

onMounted(async () => {
  meta.value = await files.read(Number(props.id))
  path.value = meta.value.path
  description.value = meta.value.description ?? ''
})

async function save() {
  error.value = ''
  try {
    await files.update(Number(props.id), { path: path.value, description: description.value || null })
    router.push('/files')
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'Save failed'
  }
}
</script>

<template>
  <div v-if="meta" class="space-y-4 max-w-xl">
    <div class="flex items-center justify-between">
      <h1 class="text-xl font-semibold">Edit file</h1>
      <div class="space-x-2 text-sm">
        <router-link to="/files" class="text-fg-2 hover:underline">Cancel</router-link>
        <button class="rounded button-primary px-3 py-1.5" @click="save">
          Save
        </button>
      </div>
    </div>
    <p v-if="error" class="text-danger text-sm">{{ error }}</p>
    <div class="bg-surface rounded shadow p-4 space-y-3">
      <div class="flex justify-center bg-surface-raised rounded p-2">
        <img
          v-if="meta.has_thumbnail"
          :src="`/files/${meta.hash}/nahled`"
          :alt="meta.title"
          class="max-h-48"
        />
        <div v-else class="text-xs text-fg-4 p-4">{{ meta.mimetype }}</div>
      </div>
      <div class="block">
        <label for="file-edit-path" class="text-sm text-fg-2">Path</label>
        <div class="mt-1">
          <PathPicker id="file-edit-path" v-model="path" namespace="file" />
        </div>
      </div>
      <label class="block">
        <span class="text-sm text-fg-2">Description</span>
        <textarea
          v-model="description"
          rows="3"
          class="mt-1 w-full rounded border border-line-1 px-2 py-1.5"
        ></textarea>
      </label>
      <dl class="text-xs text-fg-3 grid grid-cols-2 gap-1">
        <dt>Mimetype</dt>
        <dd>{{ meta.mimetype }}</dd>
        <dt>Size</dt>
        <dd>{{ meta.size_bytes }} bytes</dd>
        <dt>Uploaded</dt>
        <dd>{{ meta.created_at }}</dd>
      </dl>
    </div>
  </div>
</template>
