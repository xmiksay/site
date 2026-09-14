<script setup lang="ts">
import { ref, computed } from 'vue'
import { useFilesStore } from '../stores/files'
import PathPicker from './PathPicker.vue'

const emit = defineEmits<{ uploaded: [id: number] }>()
const files = useFilesStore()

type Mode = 'file' | 'text'
const mode = ref<Mode>('file')
const file = ref<File | null>(null)
const path = ref('')
const description = ref('')
const submitting = ref(false)
const error = ref('')

const text = ref('')
const filename = ref('')

const mimeByExt: Record<string, string> = {
  pgn: 'application/vnd.chess-pgn',
  fen: 'application/x-chess-fen',
  txt: 'text/plain',
  md: 'text/markdown',
  json: 'application/json',
  csv: 'text/csv',
  xml: 'application/xml',
  yaml: 'application/yaml',
  yml: 'application/yaml',
}

function guessMime(name: string): string {
  const ext = name.split('.').pop()?.toLowerCase() ?? ''
  return mimeByExt[ext] ?? 'text/plain'
}

const canSubmit = computed(() => {
  if (submitting.value || !path.value) return false
  return mode.value === 'file'
    ? file.value !== null
    : text.value.length > 0 && filename.value.length > 0
})

function pickFile(e: Event) {
  const target = e.target as HTMLInputElement
  file.value = target.files?.[0] ?? null
  if (file.value && !path.value) {
    path.value = file.value.name
  }
}

function onFilenameInput() {
  if (filename.value && !path.value) {
    path.value = filename.value
  }
}

async function submit() {
  if (!canSubmit.value) return
  error.value = ''
  submitting.value = true
  try {
    const upload =
      mode.value === 'file'
        ? file.value!
        : new File([text.value], filename.value, { type: guessMime(filename.value) })
    const created = await files.upload(upload, path.value, description.value || null)
    file.value = null
    text.value = ''
    filename.value = ''
    path.value = ''
    description.value = ''
    emit('uploaded', created.id)
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'Upload failed'
  } finally {
    submitting.value = false
  }
}
</script>

<template>
  <form class="bg-surface shadow rounded p-3 space-y-2" @submit.prevent="submit">
    <div class="flex gap-1 text-sm">
      <button
        type="button"
        :class="[
          'px-2 py-1 rounded',
          mode === 'file' ? 'bg-primary text-fg-inverse' : 'bg-surface-raised text-fg-2',
        ]"
        @click="mode = 'file'"
      >
        File
      </button>
      <button
        type="button"
        :class="[
          'px-2 py-1 rounded',
          mode === 'text' ? 'bg-primary text-fg-inverse' : 'bg-surface-raised text-fg-2',
        ]"
        @click="mode = 'text'"
      >
        Paste text
      </button>
    </div>
    <input v-if="mode === 'file'" type="file" @change="pickFile" />
    <template v-else>
      <input
        v-model="filename"
        placeholder="Filename (e.g. game.pgn, position.fen)"
        class="w-full rounded border border-line-1 px-2 py-1.5 text-sm"
        @input="onFilenameInput"
      />
      <textarea
        v-model="text"
        placeholder="Paste content here…"
        rows="8"
        class="w-full rounded border border-line-1 px-2 py-1.5 text-sm font-mono"
      ></textarea>
    </template>
    <PathPicker v-model="path" namespace="file" placeholder="Path (e.g. notes/2026/game.pgn)" />
    <input
      v-model="description"
      placeholder="Description (optional)"
      class="w-full rounded border border-line-1 px-2 py-1.5 text-sm"
    />
    <p v-if="error" class="text-danger text-sm">{{ error }}</p>
    <button
      :disabled="!canSubmit"
      class="rounded button-primary px-3 py-1.5 text-sm disabled:opacity-50"
    >
      {{ submitting ? 'Uploading…' : 'Upload' }}
    </button>
  </form>
</template>
