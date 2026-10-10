<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { ApiError } from '../api'
import { useDesignStore } from '../stores/design'
import { designFileUrl, fileStatus, isImagePath, isTextPath } from '../lib/designPaths'
import type { DesignSource } from '../lib/designPaths'
import { formatBytes } from '../lib/format'
import type { DesignFile } from '../types'

const props = defineProps<{ file: DesignFile }>()
const emit = defineEmits<{ dirty: [value: boolean] }>()

const design = useDesignStore()
const isText = computed(() => isTextPath(props.file.path))
const isImage = computed(() => isImagePath(props.file.path))
const status = computed(() => fileStatus(props.file))

const loading = ref(false)
const busy = ref(false)
const error = ref('')
const content = ref('')
const original = ref('')
// A baked file the admin chose to override; the override exists only after Save.
const pendingOverride = ref(false)
// Derived from the file's own state, so a draft change made elsewhere (the AI
// overriding or reverting this file) flips it without a reload.
const editing = computed(() => props.file.overridden || pendingOverride.value)
const previewUrl = ref<string | null>(null)
// 'published' / 'baked' show that version read-only beside the draft; the
// draft edit stays untouched underneath.
const source = ref<DesignSource>('draft')
const sourceText = ref('')
const sourceUrl = ref<string | null>(null)
const sourceMissing = ref(false)

const dirty = computed(
  () => pendingOverride.value || (editing.value && content.value !== original.value),
)
// Sync, so `editing` and `dirty` never see the flip half-applied: an override
// appearing ends a pending one; one reverted under unsaved edits keeps them
// editable as a pending override.
watch(
  () => props.file.overridden,
  (overridden) => {
    if (overridden) pendingOverride.value = false
    else if (content.value !== original.value) pendingOverride.value = true
  },
  { flush: 'sync' },
)
watch(dirty, (d) => emit('dirty', d), { immediate: true })

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

async function run(action: () => Promise<void>) {
  busy.value = true
  error.value = ''
  try {
    await action()
  } catch (e) {
    error.value = message(e)
  } finally {
    busy.value = false
  }
}

function setPreview(blob: Blob | null) {
  if (previewUrl.value) URL.revokeObjectURL(previewUrl.value)
  previewUrl.value = blob ? URL.createObjectURL(blob) : null
}

function setSourceUrl(blob: Blob | null) {
  if (sourceUrl.value) URL.revokeObjectURL(sourceUrl.value)
  sourceUrl.value = blob ? URL.createObjectURL(blob) : null
}

async function loadSource() {
  if (source.value === 'draft') return
  sourceMissing.value = false
  try {
    const blob = await design.fetchContent(props.file.path, source.value)
    if (isText.value) sourceText.value = await blob.text()
    else setSourceUrl(blob)
  } catch (e) {
    if (!(e instanceof ApiError && e.status === 404)) throw e
    sourceMissing.value = true
    sourceText.value = ''
    setSourceUrl(null)
  }
}
watch(source, () => run(loadSource))

// Another surface (the AI, another tab) changed the draft: pick up this
// file's new content unless the admin is editing it.
watch(
  () => design.revision,
  async () => {
    if (dirty.value || busy.value || loading.value) return
    try {
      if (isText.value) {
        const text = await design.fetchText(props.file.path)
        if (!dirty.value && text !== original.value) {
          content.value = text
          original.value = text
        }
      } else if (isImage.value) {
        setPreview(await design.fetchContent(props.file.path))
      }
      await loadSource()
    } catch {
      // The file may be gone from the draft; the parent unmounts us then.
    }
  },
)

async function load() {
  loading.value = true
  error.value = ''
  try {
    if (isText.value) {
      content.value = await design.fetchText(props.file.path)
      original.value = content.value
    } else if (isImage.value) {
      setPreview(await design.fetchContent(props.file.path))
    }
  } catch (e) {
    error.value = message(e)
  } finally {
    loading.value = false
  }
}

function startOverride() {
  return run(async () => {
    content.value = await design.fetchText(props.file.path, 'baked')
    original.value = content.value
    pendingOverride.value = true
  })
}

function cancelOverride() {
  pendingOverride.value = false
  return load()
}

function save() {
  if (!editing.value || busy.value) return
  return run(async () => {
    const body = content.value
    await design.save(props.file.path, body)
    original.value = body
    pendingOverride.value = false
  })
}

function removeOverride() {
  const baked = props.file.baked
  const what = baked ? 'Revert to the baked default' : 'Delete this override-only file'
  if (!confirm(`${what}? The override for ${props.file.path} will be removed.`)) return
  return run(async () => {
    await design.remove(props.file.path)
    // An override-only file disappears from the state and the parent unmounts us.
    if (baked) await load()
  })
}

function replace(e: Event) {
  const input = e.target as HTMLInputElement
  const picked = input.files?.[0]
  input.value = ''
  if (!picked) return
  return run(async () => {
    await design.save(props.file.path, picked)
    if (isImage.value) setPreview(await design.fetchContent(props.file.path))
  })
}

onMounted(load)
onBeforeUnmount(() => {
  setPreview(null)
  setSourceUrl(null)
  emit('dirty', false)
})
</script>

<template>
  <div class="bg-surface rounded shadow p-3 space-y-3">
    <div class="flex flex-wrap items-center gap-2">
      <h2 class="font-mono text-sm font-semibold break-all">{{ file.path }}</h2>
      <span class="text-xs rounded px-1.5 py-0.5 bg-surface-raised text-fg-2">{{ status }}</span>
      <span class="text-xs text-fg-3">{{ formatBytes(file.size) }}</span>
      <span v-if="dirty" class="text-xs text-warning">● unsaved changes</span>
      <select v-model="source" class="ml-auto rounded border border-line-1 px-1 py-0.5 text-xs" aria-label="Version">
        <option value="draft">Draft (editable)</option>
        <option value="published">Published</option>
        <option value="baked">Baked default</option>
      </select>
    </div>

    <p v-if="error" class="text-sm text-danger-strong bg-danger-bg border border-danger-soft rounded p-2 whitespace-pre-wrap font-mono">
      {{ error }}
    </p>
    <p v-if="loading" class="text-sm text-fg-4">Loading…</p>

    <template v-else-if="source !== 'draft'">
      <p v-if="sourceMissing" class="text-sm text-fg-3">Not in the {{ source }} design.</p>
      <textarea
        v-else-if="isText"
        :value="sourceText"
        readonly
        spellcheck="false"
        rows="28"
        class="w-full rounded border border-line-1 px-2 py-1.5 text-xs font-mono leading-relaxed bg-surface-alt text-fg-2"
        data-test="source-view"
      ></textarea>
      <img v-else-if="isImage && sourceUrl" :src="sourceUrl" :alt="`${file.path} (${source})`" class="max-h-80 max-w-full" />
      <p v-else class="text-sm text-fg-3">Binary file — no inline view.</p>
    </template>

    <template v-else-if="isText">
      <textarea
        v-model="content"
        :readonly="!editing"
        spellcheck="false"
        rows="28"
        class="w-full rounded border border-line-1 px-2 py-1.5 text-xs font-mono leading-relaxed"
        :class="{ 'bg-surface-alt text-fg-2': !editing }"
        @keydown.ctrl.s.prevent="save"
        @keydown.meta.s.prevent="save"
      ></textarea>
    </template>

    <template v-else>
      <img
        v-if="isImage && previewUrl"
        :src="previewUrl"
        :alt="file.path"
        class="max-h-80 max-w-full border border-line-2 bg-[repeating-conic-gradient(#eee_0_25%,#fff_0_50%)] bg-[length:16px_16px]"
      />
      <p v-else class="text-sm text-fg-3">Binary file — no inline editor.</p>
    </template>

    <div v-if="source === 'draft'" class="flex flex-wrap items-center gap-2 text-sm">
      <a
        :href="designFileUrl(file.path)"
        :download="file.path.split('/').pop()"
        class="text-accent hover:underline"
      >
        Download
      </a>
      <template v-if="isText">
        <button
          v-if="!editing && status === 'baked'"
          :disabled="busy"
          class="rounded button-primary px-3 py-1.5 disabled:opacity-50"
          @click="startOverride"
        >
          Override
        </button>
        <button
          v-if="editing"
          :disabled="busy || !dirty"
          class="rounded button-primary px-3 py-1.5 disabled:opacity-50"
          @click="save"
        >
          {{ busy ? 'Saving…' : 'Save' }}
        </button>
        <button
          v-if="editing && !file.overridden"
          :disabled="busy"
          class="rounded border border-line-1 px-3 py-1.5 hover:bg-surface-raised"
          @click="cancelOverride"
        >
          Cancel
        </button>
      </template>
      <label v-else class="rounded border border-line-1 px-3 py-1.5 hover:bg-surface-raised cursor-pointer">
        Replace…
        <input type="file" class="hidden" :disabled="busy" @change="replace" />
      </label>
      <button
        v-if="file.overridden"
        :disabled="busy"
        class="ml-auto text-danger hover:underline disabled:opacity-50"
        @click="removeOverride"
      >
        {{ file.baked ? 'Revert to baked' : 'Delete override' }}
      </button>
    </div>
  </div>
</template>
