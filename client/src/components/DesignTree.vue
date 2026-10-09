<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { DesignFile } from '../types'
import { DESIGN_ROOTS, ancestorFolders, fileStatus, visibleRows } from '../lib/designPaths'
import type { DesignTreeRow } from '../lib/designPaths'

const props = defineProps<{
  files: DesignFile[]
  selected: string | null
  folder: string
}>()
const emit = defineEmits<{ select: [path: string]; folder: [path: string] }>()

const expanded = ref(new Set<string>(DESIGN_ROOTS))
const rows = computed(() => visibleRows(props.files, expanded.value))

watch(
  () => props.selected,
  (path) => {
    if (!path) return
    const next = new Set(expanded.value)
    for (const a of ancestorFolders(path)) next.add(a)
    expanded.value = next
  },
  { immediate: true },
)

function clickRow(row: DesignTreeRow) {
  if (row.kind === 'file') {
    emit('select', row.path)
    return
  }
  const next = new Set(expanded.value)
  if (next.has(row.path)) next.delete(row.path)
  else next.add(row.path)
  expanded.value = next
  emit('folder', row.path)
}

const badge: Record<string, { label: string; cls: string }> = {
  overridden: { label: 'override', cls: 'bg-amber-100 text-amber-800' },
  'override-only': { label: 'custom', cls: 'bg-emerald-100 text-emerald-800' },
}
</script>

<template>
  <ul class="text-sm font-mono">
    <li v-for="row in rows" :key="row.path">
      <button
        type="button"
        class="w-full flex items-center gap-1 text-left px-2 py-1 rounded hover:bg-gray-100"
        :class="{
          'bg-blue-50 text-blue-800': row.kind === 'file' && row.path === selected,
          'font-semibold': row.kind === 'folder' && row.path === folder,
        }"
        :style="{ paddingLeft: `${0.5 + row.depth * 1}rem` }"
        :title="row.path"
        @click="clickRow(row)"
      >
        <span class="w-3 text-gray-400 shrink-0">
          {{ row.kind === 'folder' ? (expanded.has(row.path) ? '▾' : '▸') : '' }}
        </span>
        <span class="truncate">{{ row.name }}{{ row.kind === 'folder' ? '/' : '' }}</span>
        <span
          v-if="row.file && badge[fileStatus(row.file)]"
          class="ml-auto shrink-0 rounded px-1.5 text-[10px] font-sans"
          :class="badge[fileStatus(row.file)].cls"
        >
          {{ badge[fileStatus(row.file)].label }}
        </span>
      </button>
    </li>
    <li v-if="rows.length === 0" class="px-2 py-1 text-gray-400 font-sans">No design files.</li>
  </ul>
</template>
