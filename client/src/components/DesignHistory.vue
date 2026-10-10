<script setup lang="ts">
// Every published version, newest first; restoring copies one into the
// draft (never straight to live) — publishing it is a separate step.
import { onMounted, ref } from 'vue'
import { useDesignStore } from '../stores/design'

// `dirty`: the open editor has unsaved edits, which a restore discards.
const props = defineProps<{ dirty: boolean }>()
const emit = defineEmits<{ restored: [id: string]; close: [] }>()

const design = useDesignStore()
const loading = ref(true)
const busy = ref<string | null>(null)
const error = ref('')

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

async function restore(id: string) {
  const changes = design.state?.changes.length ?? 0
  const lost = changes ? ` The ${changes} unpublished change(s) in the draft are replaced.` : ''
  const unsaved = props.dirty ? " The open file's unsaved edits are lost too." : ''
  if (!confirm(`Restore version ${id} into the draft?${lost}${unsaved}`)) return
  busy.value = id
  error.value = ''
  try {
    await design.restore(id)
    emit('restored', id)
  } catch (e) {
    error.value = message(e)
  } finally {
    busy.value = null
  }
}

onMounted(async () => {
  try {
    await design.loadHistory()
  } catch (e) {
    error.value = message(e)
  } finally {
    loading.value = false
  }
})
</script>

<template>
  <section class="bg-surface rounded shadow p-3 text-sm">
    <div class="flex items-center justify-between mb-2">
      <h2 class="font-semibold">Historie</h2>
      <button type="button" class="text-fg-3 hover:text-fg-1" aria-label="Close" @click="emit('close')">
        ✕
      </button>
    </div>
    <p v-if="error" class="text-danger mb-2">{{ error }}</p>
    <p v-if="loading" class="text-fg-4">Loading…</p>
    <p v-else-if="design.history.length === 0" class="text-fg-3">Nothing published yet.</p>
    <ul v-else class="divide-y max-h-64 overflow-auto">
      <li v-for="(h, i) in design.history" :key="h.id" class="flex flex-wrap items-center gap-2 py-1.5">
        <span class="font-medium">{{ new Date(h.at).toLocaleString() }}</span>
        <span class="text-fg-3">by {{ h.by }} · {{ h.files }} files</span>
        <span v-if="i === 0" class="text-xs rounded px-1.5 bg-success-bg text-success-strong">latest</span>
        <button
          type="button"
          class="ml-auto text-accent hover:underline disabled:opacity-50"
          :disabled="busy !== null"
          @click="restore(h.id)"
        >
          {{ busy === h.id ? 'Restoring…' : 'Restore to draft' }}
        </button>
      </li>
    </ul>
  </section>
</template>
