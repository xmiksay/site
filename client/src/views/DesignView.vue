<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { onBeforeRouteLeave } from 'vue-router'
import { ApiError } from '../api'
import { useDesignStore } from '../stores/design'
import { parentFolder } from '../lib/designPaths'
import DesignTree from '../components/DesignTree.vue'
import DesignEditor from '../components/DesignEditor.vue'
import DesignUpload from '../components/DesignUpload.vue'

const design = useDesignStore()
const selected = ref<string | null>(null)
const folder = ref('templates')
const dirty = ref(false)
const reloading = ref(false)
const error = ref('')
// Bumped after a storage reload to remount the editor with fresh content.
const editorGeneration = ref(0)

const selectedFile = computed(() => design.files.find((f) => f.path === selected.value) ?? null)
const lastReload = computed(() => design.state?.last_reload ?? null)

watch(selectedFile, (f) => {
  if (!f && selected.value) selected.value = null
})

function confirmDiscard(): boolean {
  return !dirty.value || confirm('Discard unsaved changes?')
}

function select(path: string) {
  if (path === selected.value || !confirmDiscard()) return
  dirty.value = false
  selected.value = path
  folder.value = parentFolder(path)
}

async function reload() {
  reloading.value = true
  error.value = ''
  try {
    await design.reload()
    if (!dirty.value) editorGeneration.value++
  } catch (e) {
    error.value = message(e)
  } finally {
    reloading.value = false
  }
}

const publishing = ref(false)
const notice = ref('')
const changeCount = computed(() => design.state?.changes.length ?? 0)

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

// Interim controls until the Design studio (#119) replaces this page.
async function publish() {
  if (dirty.value && !confirm('Unsaved editor changes are not part of the draft. Publish anyway?')) return
  publishing.value = true
  error.value = ''
  notice.value = ''
  try {
    let entry
    try {
      entry = await design.publish()
    } catch (e) {
      // 409 with a force hint: design/ changed outside the draft.
      const conflict = e instanceof ApiError && e.status === 409 && e.message.includes('force')
      if (!conflict) throw e
      if (!confirm(`${e.message}\n\nPublish anyway and overwrite those changes?`)) return
      entry = await design.publish(true)
    }
    notice.value = `Published version ${entry.id}.`
  } catch (e) {
    error.value = message(e)
  } finally {
    publishing.value = false
  }
}

async function discard() {
  if (!confirm('Discard every draft change and reset the draft to the live design?')) return
  publishing.value = true
  error.value = ''
  notice.value = ''
  try {
    await design.discard()
    dirty.value = false
    editorGeneration.value++
  } catch (e) {
    error.value = message(e)
  } finally {
    publishing.value = false
  }
}

function onUploaded(path: string) {
  if (path === selected.value) editorGeneration.value++
  else select(path)
}

function warnUnload(e: BeforeUnloadEvent) {
  if (dirty.value) e.preventDefault()
}

onBeforeRouteLeave(() => confirmDiscard())
onMounted(async () => {
  window.addEventListener('beforeunload', warnUnload)
  try {
    await design.load()
  } catch (e) {
    error.value = message(e)
  }
})
onBeforeUnmount(() => window.removeEventListener('beforeunload', warnUnload))
</script>

<template>
  <div class="space-y-4">
    <div class="flex flex-wrap items-center gap-3">
      <h1 class="text-xl font-semibold">Design</h1>
      <span v-if="design.state" class="text-xs rounded px-2 py-0.5 bg-gray-200 text-gray-700">
        storage: {{ design.state.storage }}
      </span>
      <button
        :disabled="reloading"
        class="rounded bg-gray-800 hover:bg-gray-700 text-white px-3 py-1.5 text-sm disabled:opacity-50"
        @click="reload"
      >
        {{ reloading ? 'Reloading…' : 'Reload' }}
      </button>
      <span v-if="lastReload" class="text-xs" :class="lastReload.ok ? 'text-gray-500' : 'text-red-600'">
        Last reload {{ new Date(lastReload.at).toLocaleString() }}:
        <template v-if="lastReload.ok">{{ lastReload.files }} published file(s)</template>
        <template v-else>failed — {{ lastReload.error }}</template>
      </span>
    </div>

    <div v-if="design.state" class="flex flex-wrap items-center gap-3 text-sm">
      <span class="text-xs rounded px-2 py-0.5 bg-amber-100 text-amber-800">Draft — not live</span>
      <span class="text-gray-600">{{ changeCount }} unpublished change(s)</span>
      <button
        :disabled="publishing || changeCount === 0"
        class="rounded bg-gray-800 hover:bg-gray-700 text-white px-3 py-1.5 disabled:opacity-50"
        @click="publish"
      >
        {{ publishing ? 'Working…' : 'Publish' }}
      </button>
      <button
        :disabled="publishing || changeCount === 0"
        class="rounded border border-gray-300 px-3 py-1.5 hover:bg-gray-50 disabled:opacity-50"
        @click="discard"
      >
        Discard draft
      </button>
      <span v-if="notice" class="text-green-700">{{ notice }}</span>
    </div>

    <p v-if="error" class="text-sm text-red-700 bg-red-50 border border-red-200 rounded p-2 whitespace-pre-wrap">
      {{ error }}
    </p>
    <p
      v-if="design.state?.local_dir"
      class="text-sm text-blue-800 bg-blue-50 border border-blue-200 rounded p-2"
    >
      DESIGN_DIR is set: files in that local folder take precedence over the published design.
    </p>

    <div v-if="design.state" class="grid gap-4 md:grid-cols-[minmax(14rem,20rem)_1fr]">
      <div class="space-y-4">
        <div class="bg-white rounded shadow p-2 max-h-[70vh] overflow-auto">
          <DesignTree
            :files="design.files"
            :selected="selected"
            :folder="folder"
            @select="select"
            @folder="folder = $event"
          />
        </div>
        <DesignUpload :folder="folder" @uploaded="onUploaded" />
      </div>
      <DesignEditor
        v-if="selectedFile"
        :key="`${selectedFile.path}:${editorGeneration}`"
        :file="selectedFile"
        @dirty="dirty = $event"
      />
      <p v-else class="text-gray-400 text-sm">Select a file to view or edit it in the draft.</p>
    </div>
  </div>
</template>
