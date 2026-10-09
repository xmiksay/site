<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useDesignStore } from '../stores/design'
import { designFileUrl, fileStatus, isImagePath, isTextPath } from '../lib/designPaths'
import { formatBytes } from '../lib/format'
import type { DesignFile } from '../types'

const props = defineProps<{ file: DesignFile; editable: boolean }>()
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
// True once the textarea is writable: an existing override, or a baked file the
// user chose to override (the override does not exist until Save).
const editing = ref(false)
const previewUrl = ref<string | null>(null)

const dirty = computed(
  () => editing.value && (content.value !== original.value || !props.file.overridden),
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

async function load() {
  loading.value = true
  error.value = ''
  try {
    if (isText.value) {
      content.value = await design.fetchText(props.file.path)
      original.value = content.value
      editing.value = props.editable && props.file.overridden
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
    content.value = await design.fetchText(props.file.path, true)
    original.value = content.value
    editing.value = true
  })
}

function cancelOverride() {
  editing.value = false
  return load()
}

function save() {
  if (!editing.value || busy.value) return
  return run(async () => {
    const body = content.value
    await design.save(props.file.path, body)
    original.value = body
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
  emit('dirty', false)
})
</script>

<template>
  <div class="bg-white rounded shadow p-3 space-y-3">
    <div class="flex flex-wrap items-center gap-2">
      <h2 class="font-mono text-sm font-semibold break-all">{{ file.path }}</h2>
      <span class="text-xs rounded px-1.5 py-0.5 bg-gray-100 text-gray-700">{{ status }}</span>
      <span class="text-xs text-gray-500">{{ formatBytes(file.size) }}</span>
      <span v-if="dirty" class="text-xs text-amber-700">● unsaved changes</span>
    </div>

    <p v-if="error" class="text-sm text-red-700 bg-red-50 border border-red-200 rounded p-2 whitespace-pre-wrap font-mono">
      {{ error }}
    </p>
    <p v-if="loading" class="text-sm text-gray-400">Loading…</p>

    <template v-else-if="isText">
      <textarea
        v-model="content"
        :readonly="!editing"
        spellcheck="false"
        rows="28"
        class="w-full rounded border border-gray-300 px-2 py-1.5 text-xs font-mono leading-relaxed"
        :class="{ 'bg-gray-50 text-gray-600': !editing }"
        @keydown.ctrl.s.prevent="save"
        @keydown.meta.s.prevent="save"
      ></textarea>
    </template>

    <template v-else>
      <img
        v-if="isImage && previewUrl"
        :src="previewUrl"
        :alt="file.path"
        class="max-h-80 max-w-full border border-gray-200 bg-[repeating-conic-gradient(#eee_0_25%,#fff_0_50%)] bg-[length:16px_16px]"
      />
      <p v-else class="text-sm text-gray-500">Binary file — no inline editor.</p>
    </template>

    <div class="flex flex-wrap items-center gap-2 text-sm">
      <a
        :href="designFileUrl(file.path)"
        :download="file.path.split('/').pop()"
        class="text-blue-600 hover:underline"
      >
        Download
      </a>
      <template v-if="editable">
        <template v-if="isText">
          <button
            v-if="!editing && status === 'baked'"
            :disabled="busy"
            class="rounded bg-gray-800 hover:bg-gray-700 text-white px-3 py-1.5 disabled:opacity-50"
            @click="startOverride"
          >
            Override
          </button>
          <button
            v-if="editing"
            :disabled="busy || !dirty"
            class="rounded bg-gray-800 hover:bg-gray-700 text-white px-3 py-1.5 disabled:opacity-50"
            @click="save"
          >
            {{ busy ? 'Saving…' : 'Save' }}
          </button>
          <button
            v-if="editing && !file.overridden"
            :disabled="busy"
            class="rounded border border-gray-300 px-3 py-1.5 hover:bg-gray-50"
            @click="cancelOverride"
          >
            Cancel
          </button>
        </template>
        <label v-else class="rounded border border-gray-300 px-3 py-1.5 hover:bg-gray-50 cursor-pointer">
          Replace…
          <input type="file" class="hidden" :disabled="busy" @change="replace" />
        </label>
        <button
          v-if="file.overridden"
          :disabled="busy"
          class="ml-auto text-red-600 hover:underline disabled:opacity-50"
          @click="removeOverride"
        >
          {{ file.baked ? 'Revert to baked' : 'Delete override' }}
        </button>
      </template>
    </div>
  </div>
</template>
