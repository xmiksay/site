<script setup lang="ts">
// The public site rendered with the draft (#116 draft preview) in an iframe.
// Same origin, so the iframe carries the session + `design_preview` cookies
// and its location can be read back into the path field as the admin clicks
// around inside it.
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useDesignStore } from '../stores/design'
import { previewPath } from '../lib/designPaths'

const REFRESH_DEBOUNCE_MS = 400

const design = useDesignStore()
const frame = ref<HTMLIFrameElement | null>(null)
const ready = ref(false)
const error = ref('')
const src = ref('/')
const input = ref('/')
// Remounts the iframe when reloading its current document is not possible.
const frameKey = ref(0)

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

async function enablePreview() {
  error.value = ''
  try {
    await design.setPreview(true)
  } catch (e) {
    error.value = `Could not turn the draft preview on: ${message(e)}`
  }
}

function currentPath(): string | null {
  try {
    const loc = frame.value?.contentWindow?.location
    return loc && loc.pathname ? `${loc.pathname}${loc.search}${loc.hash}` : null
  } catch {
    return null
  }
}

function onLoad() {
  const path = currentPath()
  if (path) input.value = path
}

// The frame may have navigated away from `src`, so both act on its live
// location; remounting at a path is the fallback.
function remountAt(path: string) {
  src.value = path
  frameKey.value++
}

function reload() {
  try {
    frame.value!.contentWindow!.location.reload()
  } catch {
    remountAt(currentPath() ?? src.value)
  }
}

function go() {
  const path = previewPath(input.value)
  input.value = path
  try {
    frame.value!.contentWindow!.location.assign(path)
  } catch {
    remountAt(path)
  }
}

// The banner's exit link inside the iframe turns the preview off; the manual
// refresh turns it back on.
async function refresh() {
  await enablePreview()
  reload()
}

function openInTab() {
  window.open(previewPath(currentPath() ?? input.value), '_blank', 'noopener')
}

let timer: ReturnType<typeof setTimeout> | null = null
watch(
  () => design.revision,
  () => {
    if (timer) clearTimeout(timer)
    timer = setTimeout(() => {
      timer = null
      if (ready.value) reload()
    }, REFRESH_DEBOUNCE_MS)
  },
)

onMounted(async () => {
  await enablePreview()
  ready.value = true
})
onBeforeUnmount(() => {
  if (timer) clearTimeout(timer)
})
</script>

<template>
  <div class="flex flex-col min-h-0 flex-1">
    <form class="p-2 border-b flex items-center gap-2 text-sm" @submit.prevent="go">
      <input
        v-model="input"
        class="min-w-0 flex-1 rounded border border-line-1 px-2 py-1 font-mono"
        aria-label="Preview path"
        placeholder="/"
      />
      <button type="submit" class="rounded border border-line-1 px-2 py-1 hover:bg-surface-raised">Go</button>
      <button
        type="button"
        class="rounded border border-line-1 px-2 py-1 hover:bg-surface-raised"
        title="Reload the preview (and turn draft preview back on)"
        @click="refresh"
      >
        ↻
      </button>
      <button
        type="button"
        class="rounded border border-line-1 px-2 py-1 hover:bg-surface-raised"
        title="Open the preview in a new tab"
        @click="openInTab"
      >
        ↗
      </button>
    </form>
    <p v-if="error" class="m-2 text-sm text-danger-strong bg-danger-bg border border-danger-soft rounded p-2">{{ error }}</p>
    <iframe
      v-if="ready"
      :key="frameKey"
      ref="frame"
      :src="src"
      title="Draft preview"
      class="flex-1 w-full min-h-[24rem] bg-white"
      @load="onLoad"
    ></iframe>
  </div>
</template>
