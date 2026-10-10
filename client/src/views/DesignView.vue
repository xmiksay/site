<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { onBeforeRouteLeave } from 'vue-router'
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
    error.value = e instanceof Error ? e.message : String(e)
  } finally {
    reloading.value = false
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
    error.value = e instanceof Error ? e.message : String(e)
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
        <template v-if="lastReload.ok">{{ lastReload.files }} override file(s)</template>
        <template v-else>failed — {{ lastReload.error }}</template>
      </span>
    </div>

    <p v-if="error" class="text-sm text-red-700 bg-red-50 border border-red-200 rounded p-2">
      {{ error }}
    </p>
    <p
      v-if="design.state?.local_dir"
      class="text-sm text-blue-800 bg-blue-50 border border-blue-200 rounded p-2"
    >
      DESIGN_DIR is set: files in that local folder take precedence over storage overrides.
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
      <p v-else class="text-gray-400 text-sm">Select a file to view or override it.</p>
    </div>
  </div>
</template>
