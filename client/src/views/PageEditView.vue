<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { usePagesStore } from '../stores/pages'
import { useTagsStore } from '../stores/tags'
import PathPicker from '../components/PathPicker.vue'
import MarkdownEditor from '../components/MarkdownEditor.vue'
import { html as diff2htmlHtml } from 'diff2html'
import 'diff2html/bundles/css/diff2html.min.css'
import type { PageInput } from '../types'

const props = defineProps<{ id?: string; create?: boolean }>()

const router = useRouter()
const pages = usePagesStore()
const tags = useTagsStore()

const path = ref('')
const summary = ref<string>('')
const markdown = ref('')
const tagIds = ref<number[]>([])
const isPrivate = ref(false)
const error = ref('')
const revisions = ref<Array<{ id: number; created_at: string }>>([])

const exportError = ref('')
const exporting = ref(false)

// Diff modal state
const diffOpen = ref(false)
const diffLoading = ref(false)
const diffError = ref('')
const diffHtml = ref('')
const diffRevDate = ref('')

const numericId = () => (props.id ? Number(props.id) : null)

onMounted(async () => {
  await tags.load()
  if (!props.create && props.id) {
    const detail = await pages.read(Number(props.id))
    path.value = detail.path
    summary.value = detail.summary ?? ''
    markdown.value = detail.markdown
    tagIds.value = detail.tag_ids
    isPrivate.value = detail.private
    revisions.value = detail.revisions
  }
})

function buildInput(): PageInput {
  return {
    path: path.value,
    summary: summary.value || null,
    markdown: markdown.value,
    tag_ids: tagIds.value,
    private: isPrivate.value,
  }
}

async function save() {
  error.value = ''
  try {
    if (props.create) {
      await pages.create(buildInput())
    } else if (props.id) {
      await pages.update(Number(props.id), buildInput())
    }
    router.push('/pages')
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'Save failed'
  }
}

function toggleTag(id: number) {
  const idx = tagIds.value.indexOf(id)
  if (idx === -1) tagIds.value.push(id)
  else tagIds.value.splice(idx, 1)
}

async function exportAs(format: 'pdf' | 'slides') {
  const id = numericId()
  if (!id) return
  exportError.value = ''
  exporting.value = true
  try {
    const { blob, filename } = await pages.exportPage(id, format)
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = filename
    document.body.appendChild(a)
    a.click()
    a.remove()
    URL.revokeObjectURL(url)
  } catch (e) {
    exportError.value = e instanceof Error ? e.message : 'Export failed'
  } finally {
    exporting.value = false
  }
}

async function openDiff(rev: { id: number; created_at: string }) {
  const id = numericId()
  if (!id) return
  diffOpen.value = true
  diffLoading.value = true
  diffError.value = ''
  diffHtml.value = ''
  diffRevDate.value = rev.created_at
  try {
    const detail = await pages.readRevision(id, rev.id)
    diffHtml.value = diff2htmlHtml(detail.diff, {
      drawFileList: false,
      matching: 'lines',
      outputFormat: 'line-by-line',
    })
  } catch (e) {
    diffError.value = e instanceof Error ? e.message : 'Failed to load revision'
  } finally {
    diffLoading.value = false
  }
}

function closeDiff() {
  diffOpen.value = false
}

async function restore(revId: number) {
  const id = numericId()
  if (!id) return
  if (!confirm('Restore this revision? Current content will be replaced.')) return
  await pages.restoreRevision(id, revId)
  const detail = await pages.read(id)
  markdown.value = detail.markdown
  revisions.value = detail.revisions
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h1 class="text-xl font-semibold">
        {{ props.create ? 'New page' : 'Edit page' }}
      </h1>
      <div class="space-x-2">
        <router-link to="/pages" class="text-gray-600 hover:underline text-sm">Cancel</router-link>
        <button
          v-if="!props.create"
          type="button"
          class="rounded border border-gray-300 px-3 py-1.5 text-sm hover:bg-gray-100 disabled:opacity-50"
          :disabled="exporting"
          @click="exportAs('pdf')"
        >
          Export PDF
        </button>
        <button
          v-if="!props.create"
          type="button"
          class="rounded border border-gray-300 px-3 py-1.5 text-sm hover:bg-gray-100 disabled:opacity-50"
          :disabled="exporting"
          @click="exportAs('slides')"
        >
          Export slides
        </button>
        <button class="rounded bg-gray-800 hover:bg-gray-700 text-white px-3 py-1.5 text-sm" @click="save">
          Save
        </button>
      </div>
    </div>

    <p v-if="error" class="text-red-600 text-sm">{{ error }}</p>
    <p v-if="exportError" class="text-red-600 text-sm">{{ exportError }}</p>

    <div class="bg-white rounded-lg shadow p-4 space-y-4">
      <label class="block">
        <span class="text-sm text-gray-600">Path</span>
        <PathPicker
          v-model="path"
          namespace="all"
          placeholder="obsidian/programing/rust"
          class="mt-1"
        />
      </label>
      <label class="block">
        <span class="text-sm text-gray-600">Summary</span>
        <input
          v-model="summary"
          class="mt-1 w-full rounded border border-gray-300 px-2 py-1.5"
        />
      </label>
      <div>
        <span class="text-sm text-gray-600">Tags</span>
        <div class="mt-1 flex flex-wrap gap-1">
          <button
            v-for="tag in tags.items"
            :key="tag.id"
            type="button"
            class="rounded-full px-2 py-0.5 text-xs border"
            :class="
              tagIds.includes(tag.id)
                ? 'bg-blue-600 border-blue-600 text-white'
                : 'border-gray-300 text-gray-700 hover:bg-gray-100'
            "
            @click="toggleTag(tag.id)"
          >
            {{ tag.name }}
          </button>
        </div>
      </div>
      <label class="inline-flex items-center gap-2 text-sm">
        <input v-model="isPrivate" type="checkbox" />
        Private
      </label>
      <MarkdownEditor v-model="markdown" :rows="24" />
    </div>

    <div v-if="!props.create && revisions.length" class="bg-white rounded-lg shadow p-4">
      <h2 class="font-medium mb-2">Revisions</h2>
      <ul class="text-sm space-y-1">
        <li
          v-for="r in revisions"
          :key="r.id"
          class="flex justify-between border-b border-gray-100 py-1"
        >
          <button
            type="button"
            class="flex-1 text-left text-gray-600 hover:text-gray-900 hover:underline"
            @click="openDiff(r)"
          >
            {{ r.created_at }}
          </button>
          <button class="text-blue-600 hover:underline" @click.stop="restore(r.id)">Restore</button>
        </li>
      </ul>
    </div>

    <div
      v-if="diffOpen"
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      @click.self="closeDiff"
    >
      <div class="flex max-h-[85vh] w-[min(900px,94vw)] flex-col overflow-hidden rounded-lg bg-white shadow-lg">
        <div class="flex items-center justify-between border-b border-gray-200 px-4 py-3">
          <h2 class="font-medium">Revision {{ diffRevDate }}</h2>
          <button class="text-gray-500 hover:text-gray-900" @click="closeDiff">×</button>
        </div>
        <div class="overflow-auto p-4">
          <p v-if="diffLoading" class="text-sm text-gray-500">Loading…</p>
          <p v-else-if="diffError" class="text-sm text-red-600">{{ diffError }}</p>
          <div v-else class="text-sm" v-html="diffHtml"></div>
        </div>
      </div>
    </div>
  </div>
</template>
