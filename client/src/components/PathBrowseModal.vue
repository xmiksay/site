<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { pathsApi, type FolderEntry, type LeafEntry, type PathNamespace } from '../api/paths'

const props = defineProps<{
  namespace: PathNamespace
}>()

const emit = defineEmits<{
  (e: 'close'): void
  (e: 'select-folder', prefix: string): void
  (e: 'select-leaf', value: string): void
}>()

defineOptions({ name: 'PathBrowseModal' })

const browsePrefix = ref('')
const browseFolders = ref<FolderEntry[]>([])
const browseLeaves = ref<LeafEntry[]>([])
const browseLoading = ref(false)

const breadcrumb = computed(() => {
  const items: { label: string; prefix: string }[] = [{ label: '/ root', prefix: '' }]
  if (!browsePrefix.value) return items
  const parts = browsePrefix.value.split('/').filter(Boolean)
  let acc = ''
  for (const p of parts) {
    acc += p + '/'
    items.push({ label: p, prefix: acc })
  }
  return items
})

async function loadBrowse(prefix: string) {
  browseLoading.value = true
  browsePrefix.value = prefix
  try {
    const res = await pathsApi.children({
      namespace: props.namespace,
      prefix,
      limit: 500,
    })
    browseFolders.value = res.folders
    browseLeaves.value = res.leaves
  } catch {
    browseFolders.value = []
    browseLeaves.value = []
  } finally {
    browseLoading.value = false
  }
}

onMounted(() => loadBrowse(''))

function browseDrill(folder: FolderEntry) {
  loadBrowse(browsePrefix.value + folder.name + '/')
}

function browsePickFolder() {
  emit('select-folder', browsePrefix.value)
}

function browsePickLeaf(leaf: LeafEntry) {
  emit('select-leaf', browsePrefix.value + leaf.name)
}
</script>

<template>
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-overlay"
    @click.self="emit('close')"
  >
    <div
      class="flex max-h-[80vh] w-[min(640px,92vw)] flex-col overflow-hidden rounded-lg border border-line-2 bg-surface"
      role="dialog"
      aria-label="Browse paths"
    >
      <header class="flex items-center justify-between border-b border-line-2 px-4 py-2">
        <h3 class="text-base font-medium">Browse</h3>
        <button
          class="text-2xl leading-none text-fg-3 hover:text-fg-1"
          type="button"
          @click="emit('close')"
        >
          ×
        </button>
      </header>

      <nav class="flex flex-wrap items-center gap-1 border-b border-line-2 px-4 py-2 text-sm">
        <template v-for="(c, i) in breadcrumb" :key="c.prefix">
          <button
            type="button"
            class="px-1 text-accent hover:underline"
            :class="{ 'cursor-default font-semibold text-fg-1 hover:no-underline': c.prefix === browsePrefix }"
            @click="loadBrowse(c.prefix)"
          >
            {{ c.label }}
          </button>
          <span v-if="i < breadcrumb.length - 1" class="text-fg-4">/</span>
        </template>
        <button
          type="button"
          class="ml-auto rounded border border-accent px-2 py-0.5 text-xs text-accent hover:bg-accent hover:text-fg-inverse"
          :title="'Use ' + (browsePrefix || '/') + ' as the prefix'"
          @click="browsePickFolder"
        >
          Use this folder
        </button>
      </nav>

      <div class="overflow-y-auto p-2">
        <p v-if="browseLoading" class="px-2 py-2 text-sm text-fg-3">Loading…</p>
        <p
          v-else-if="browseFolders.length === 0 && browseLeaves.length === 0"
          class="px-2 py-2 text-sm text-fg-3"
        >
          Empty folder.
        </p>
        <ul v-else class="m-0 list-none p-0">
          <li
            v-for="f in browseFolders"
            :key="'f:' + f.name"
            class="grid cursor-pointer grid-cols-[1.2rem_1fr_auto] items-center gap-2 rounded px-2 py-1.5 hover:bg-surface-raised"
            @click="browseDrill(f)"
          >
            <span class="text-center text-fg-4">▸</span>
            <span class="truncate">
              {{ f.name }}<span class="text-fg-4">/</span>
            </span>
            <span class="flex items-center gap-1 whitespace-nowrap text-xs text-fg-3">
              <span
                v-if="f.page_count"
                class="rounded-full border border-line-1 px-1.5 text-[0.65rem] uppercase"
              >
                p {{ f.page_count }}
              </span>
              <span
                v-if="f.gallery_count"
                class="rounded-full border border-line-1 px-1.5 text-[0.65rem] uppercase"
              >
                g {{ f.gallery_count }}
              </span>
              <span
                v-if="f.file_count"
                class="rounded-full border border-line-1 px-1.5 text-[0.65rem] uppercase"
              >
                f {{ f.file_count }}
              </span>
            </span>
          </li>
          <li
            v-for="l in browseLeaves"
            :key="'l:' + l.namespace + ':' + l.name"
            class="grid cursor-pointer grid-cols-[1.2rem_1fr_auto] items-center gap-2 rounded px-2 py-1.5 hover:bg-surface-raised"
            @click="browsePickLeaf(l)"
          >
            <span class="text-center text-fg-4">·</span>
            <span class="truncate">{{ l.name }}</span>
            <span class="flex items-center gap-1 whitespace-nowrap text-xs text-fg-3">
              <span class="rounded-full border border-line-1 px-1.5 text-[0.65rem] uppercase">
                {{ l.namespace }}
              </span>
              <span v-if="l.title" class="text-fg-4">{{ l.title }}</span>
            </span>
          </li>
        </ul>
      </div>
    </div>
  </div>
</template>
