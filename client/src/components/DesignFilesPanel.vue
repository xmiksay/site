<script setup lang="ts">
// The studio's file side: the draft tree with change markers, the list of
// unpublished changes (deleted files included, which the tree cannot show),
// asset upload and the editor for the selected file.
import { computed, ref, watch } from 'vue'
import { useDesignStore } from '../stores/design'
import { parentFolder } from '../lib/designPaths'
import type { DesignChange } from '../types'
import DesignTree from './DesignTree.vue'
import DesignEditor from './DesignEditor.vue'
import DesignUpload from './DesignUpload.vue'

const emit = defineEmits<{ dirty: [value: boolean] }>()

const design = useDesignStore()
const selected = ref<string | null>(null)
const folder = ref('templates')
const dirty = ref(false)
// Bumped to remount the editor with fresh content after the draft was
// replaced wholesale (discard, restore).
const editorGeneration = ref(0)

const selectedFile = computed(() => design.files.find((f) => f.path === selected.value) ?? null)
const changes = computed(() => design.state?.changes ?? [])
const paths = computed(() => new Set(design.files.map((f) => f.path)))

// A draft without its own copy of a baked file falls back to the baked
// default, which the server reports as `deleted` while the file stays in the tree.
function changeLabel(c: DesignChange): string {
  if (c.kind === 'deleted') return paths.value.has(c.path) ? 'reverted to baked' : 'deleted'
  return c.kind === 'added' ? 'new' : 'changed'
}

watch(selectedFile, (f) => {
  if (!f && selected.value) selected.value = null
})
watch(dirty, (d) => emit('dirty', d))

function confirmDiscard(): boolean {
  return !dirty.value || confirm('Discard unsaved changes?')
}

function select(path: string) {
  if (path === selected.value || !paths.value.has(path) || !confirmDiscard()) return
  dirty.value = false
  selected.value = path
  folder.value = parentFolder(path)
}

function onUploaded(path: string) {
  if (path === selected.value) editorGeneration.value++
  else select(path)
}

/** Drop unsaved editor state and reload the open file from the draft. */
function reset() {
  dirty.value = false
  editorGeneration.value++
}

defineExpose({ confirmDiscard, reset })
</script>

<template>
  <div class="flex flex-col gap-3 min-h-0 flex-1 overflow-auto">
    <div class="grid gap-3 md:grid-cols-2 xl:grid-cols-1 2xl:grid-cols-2 *:min-w-0">
      <div class="bg-white rounded shadow p-2 max-h-72 overflow-auto">
        <DesignTree
          :files="design.files"
          :selected="selected"
          :folder="folder"
          :changes="changes"
          @select="select"
          @folder="folder = $event"
        />
      </div>
      <div class="space-y-3">
        <section class="bg-white rounded shadow p-2 text-sm max-h-40 overflow-auto">
          <h2 class="font-semibold px-1">Unpublished changes ({{ changes.length }})</h2>
          <p v-if="changes.length === 0" class="px-1 text-gray-500">The draft matches the live design.</p>
          <ul v-else class="font-mono text-xs">
            <li v-for="c in changes" :key="c.path">
              <button
                type="button"
                class="w-full flex gap-2 text-left px-1 py-0.5 rounded hover:bg-gray-100 disabled:hover:bg-transparent"
                :disabled="!paths.has(c.path)"
                :class="{ 'line-through text-gray-400': !paths.has(c.path) }"
                @click="select(c.path)"
              >
                <span class="truncate">{{ c.path }}</span>
                <span class="ml-auto shrink-0 font-sans text-blue-700">{{ changeLabel(c) }}</span>
              </button>
            </li>
          </ul>
        </section>
        <DesignUpload :folder="folder" @uploaded="onUploaded" />
      </div>
    </div>
    <DesignEditor
      v-if="selectedFile"
      :key="`${selectedFile.path}:${editorGeneration}`"
      :file="selectedFile"
      @dirty="dirty = $event"
    />
    <p v-else class="text-gray-400 text-sm">Select a file to view or edit it in the draft.</p>
  </div>
</template>
