<script setup lang="ts">
import { computed, ref } from 'vue'
import { useDesignStore } from '../stores/design'
import { defaultUploadPath, validateDesignPath } from '../lib/designPaths'

// `dirtyPath`: the file open in the editor with unsaved edits, if any.
const props = defineProps<{ folder: string; dirtyPath?: string | null }>()
const emit = defineEmits<{ uploaded: [path: string] }>()

const design = useDesignStore()
const file = ref<File | null>(null)
const target = ref('')
const busy = ref(false)
const error = ref('')
const inputKey = ref(0)

const pathError = computed(() => (target.value ? validateDesignPath(target.value) : null))
const exists = computed(() => design.files.some((f) => f.path === target.value))
const canSubmit = computed(() => !busy.value && file.value && target.value && !pathError.value)

function pick(e: Event) {
  file.value = (e.target as HTMLInputElement).files?.[0] ?? null
  if (file.value) target.value = defaultUploadPath(file.value.name, props.folder)
}

async function submit() {
  if (!canSubmit.value || !file.value) return
  if (target.value === props.dirtyPath) {
    if (!confirm(`Overwrite ${target.value} and discard its unsaved edits?`)) return
  } else if (exists.value && !confirm(`Overwrite ${target.value}?`)) return
  busy.value = true
  error.value = ''
  try {
    const path = target.value
    await design.save(path, file.value)
    file.value = null
    target.value = ''
    inputKey.value++
    emit('uploaded', path)
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'Upload failed'
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <form class="bg-surface rounded shadow p-3 space-y-2 text-sm" @submit.prevent="submit">
    <h2 class="font-semibold">Upload to draft</h2>
    <input :key="inputKey" type="file" class="w-full" @change="pick" />
    <input
      v-model="target"
      placeholder="Target path (images → assets/img/, fonts → assets/fonts/)"
      class="w-full rounded border border-line-1 px-2 py-1.5 font-mono"
    />
    <p v-if="pathError" class="text-danger">{{ pathError }}</p>
    <p v-else-if="exists" class="text-warning">Replaces the existing {{ target }}.</p>
    <p v-if="error" class="text-danger whitespace-pre-wrap font-mono">{{ error }}</p>
    <button
      :disabled="!canSubmit"
      class="rounded button-primary px-3 py-1.5 disabled:opacity-50"
    >
      {{ busy ? 'Uploading…' : 'Upload' }}
    </button>
  </form>
</template>
