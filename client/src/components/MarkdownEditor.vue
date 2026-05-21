<script setup lang="ts">
import { computed, nextTick, ref } from 'vue'
import FilePicker from './FilePicker.vue'
import PathPicker from './PathPicker.vue'
import { useFilesStore } from '../stores/files'
import { useGalleriesStore } from '../stores/galleries'
import { useDebouncedRender } from '../composables/useDebouncedRender'

const props = withDefaults(
  defineProps<{
    modelValue: string
    rows?: number
    inputId?: string
    placeholder?: string
  }>(),
  { rows: 12, inputId: undefined, placeholder: '' },
)
const emit = defineEmits<{ (e: 'update:modelValue', v: string): void }>()

const textareaRef = ref<HTMLTextAreaElement | null>(null)
const tab = ref<'edit' | 'preview'>('edit')

const source = computed(() => props.modelValue)
const active = computed(() => tab.value === 'preview')
const { html: previewHtml, loading: previewLoading, error: previewError } =
  useDebouncedRender(source, active)

type PickerKind = null | 'img' | 'file' | 'gallery' | 'page' | 'pgn' | 'fen'
const pickerKind = ref<PickerKind>(null)
const pickerPagePath = ref('')
const galleryPaths = ref<string[]>([])
const galleriesLoading = ref(false)

const files = useFilesStore()
const galleries = useGalleriesStore()

interface EditResult {
  text: string
  selStart?: number
  selEnd?: number
}

function applyEdit(transform: (selected: string) => EditResult) {
  const ta = textareaRef.value
  if (!ta) return
  const start = ta.selectionStart
  const end = ta.selectionEnd
  const value = ta.value
  const before = value.slice(0, start)
  const selected = value.slice(start, end)
  const after = value.slice(end)
  const r = transform(selected)
  const newValue = before + r.text + after
  emit('update:modelValue', newValue)
  nextTick(() => {
    ta.focus()
    const s = start + (r.selStart ?? r.text.length)
    const e = start + (r.selEnd ?? r.text.length)
    ta.setSelectionRange(s, e)
  })
}

function wrap(prefix: string, suffix: string, placeholder: string) {
  applyEdit((sel) => {
    if (sel) {
      return { text: prefix + sel + suffix, selStart: prefix.length, selEnd: prefix.length + sel.length }
    }
    return {
      text: prefix + placeholder + suffix,
      selStart: prefix.length,
      selEnd: prefix.length + placeholder.length,
    }
  })
}

function applyLinePrefix(prefix: (i: number) => string) {
  const ta = textareaRef.value
  if (!ta) return
  const value = ta.value
  let start = ta.selectionStart
  let end = ta.selectionEnd
  while (start > 0 && value[start - 1] !== '\n') start--
  if (end > start && value[end - 1] === '\n') end--
  const selectedBlock = value.slice(start, end)
  const lines = selectedBlock.split('\n')
  const newBlock = lines.map((l, i) => prefix(i) + l).join('\n')
  const before = value.slice(0, start)
  const after = value.slice(end)
  const newValue = before + newBlock + after
  emit('update:modelValue', newValue)
  nextTick(() => {
    ta.focus()
    ta.setSelectionRange(start, start + newBlock.length)
  })
}

function insertHeading() {
  applyLinePrefix(() => '## ')
}
function insertList() {
  applyLinePrefix(() => '- ')
}
function insertOrderedList() {
  applyLinePrefix((i) => `${i + 1}. `)
}
function insertBlockquote() {
  applyLinePrefix(() => '> ')
}
function insertBold() {
  wrap('**', '**', 'bold')
}
function insertItalic() {
  wrap('*', '*', 'italic')
}
function insertInlineCode() {
  wrap('`', '`', 'code')
}
function insertCodeBlock() {
  applyEdit((sel) => {
    const body = sel || 'code'
    const text = `\n\`\`\`\n${body}\n\`\`\`\n`
    const selStart = 5
    const selEnd = selStart + body.length
    return { text, selStart, selEnd }
  })
}
function insertLink() {
  applyEdit((sel) => {
    const label = sel || 'text'
    const text = `[${label}](url)`
    const selStart = 1 + label.length + 2
    const selEnd = selStart + 3
    return { text, selStart, selEnd }
  })
}

function escapeAttr(v: string): string {
  return v
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

function insertDirective(name: string, args: Record<string, string>) {
  const attrs = Object.entries(args)
    .map(([k, v]) => ` ${k}="${escapeAttr(v)}"`)
    .join('')
  applyEdit(() => {
    const text = `\n<${name}${attrs}>\n`
    return { text, selStart: text.length, selEnd: text.length }
  })
}

function onKeydown(e: KeyboardEvent) {
  if (!(e.metaKey || e.ctrlKey)) return
  const key = e.key.toLowerCase()
  if (key === 'b') {
    e.preventDefault()
    insertBold()
  } else if (key === 'i') {
    e.preventDefault()
    insertItalic()
  }
}

async function openGalleryPicker() {
  pickerKind.value = 'gallery'
  if (galleryPaths.value.length === 0) {
    galleriesLoading.value = true
    try {
      galleryPaths.value = await galleries.loadPaths()
    } finally {
      galleriesLoading.value = false
    }
  }
}

function openPagePicker() {
  pickerPagePath.value = ''
  pickerKind.value = 'page'
}

function closePicker() {
  pickerKind.value = null
}

function pickFile(id: number) {
  const f = files.items.find((x) => x.id === id)
  const path = f?.path ?? ''
  const kind = pickerKind.value
  closePicker()
  if (!path || !kind) return
  if (kind === 'img') insertDirective('image', { path })
  else if (kind === 'file') insertDirective('file', { path })
  else if (kind === 'pgn') insertDirective('pgn', { path })
  else if (kind === 'fen') insertDirective('fen', { path })
}

function pickGallery(path: string) {
  closePicker()
  insertDirective('gallery', { path })
}

function confirmPagePick() {
  const path = pickerPagePath.value.trim()
  if (!path) return
  closePicker()
  insertDirective('page', { path })
}

const filePickerMime = computed(() => {
  if (pickerKind.value === 'img') return 'image/'
  return undefined
})
const fileLikePicker = computed(
  () => pickerKind.value === 'img' || pickerKind.value === 'file' || pickerKind.value === 'pgn' || pickerKind.value === 'fen',
)

const tbBtn =
  'rounded border border-gray-300 px-2 py-1 text-xs text-gray-700 hover:bg-gray-100 hover:border-gray-500'
</script>

<template>
  <div class="border border-gray-200 rounded">
    <div class="flex flex-wrap items-center gap-1 border-b border-gray-200 bg-gray-50 px-2 py-1.5">
      <button type="button" :class="tbBtn" title="Bold (Ctrl+B)" @click="insertBold"><b>B</b></button>
      <button type="button" :class="tbBtn" title="Italic (Ctrl+I)" @click="insertItalic"><i>I</i></button>
      <button type="button" :class="tbBtn" title="Heading" @click="insertHeading">H</button>
      <button type="button" :class="tbBtn" title="Bullet list" @click="insertList">•</button>
      <button type="button" :class="tbBtn" title="Ordered list" @click="insertOrderedList">1.</button>
      <button type="button" :class="tbBtn" title="Blockquote" @click="insertBlockquote">&gt;</button>
      <button type="button" :class="tbBtn" title="Link" @click="insertLink">Link</button>
      <button type="button" :class="tbBtn" title="Inline code" @click="insertInlineCode">`code`</button>
      <button type="button" :class="tbBtn" title="Code block" @click="insertCodeBlock">```</button>
      <span class="mx-1 h-4 w-px bg-gray-300"></span>
      <button type="button" :class="tbBtn" title="Insert image directive" @click="pickerKind = 'img'">&lt;image&gt;</button>
      <button type="button" :class="tbBtn" title="Insert file directive" @click="pickerKind = 'file'">&lt;file&gt;</button>
      <button type="button" :class="tbBtn" title="Insert gallery directive" @click="openGalleryPicker">&lt;gallery&gt;</button>
      <button type="button" :class="tbBtn" title="Insert page transclude" @click="openPagePicker">&lt;page&gt;</button>
      <button type="button" :class="tbBtn" title="Insert PGN chess viewer" @click="pickerKind = 'pgn'">&lt;pgn&gt;</button>
      <button type="button" :class="tbBtn" title="Insert FEN chess position" @click="pickerKind = 'fen'">&lt;fen&gt;</button>
      <span class="ml-auto flex gap-0.5 text-sm">
        <button
          type="button"
          class="px-3 py-1 rounded"
          :class="tab === 'edit' ? 'bg-white font-medium border border-gray-300' : 'text-gray-500 hover:text-gray-800'"
          @click="tab = 'edit'"
        >
          Edit
        </button>
        <button
          type="button"
          class="px-3 py-1 rounded"
          :class="tab === 'preview' ? 'bg-white font-medium border border-gray-300' : 'text-gray-500 hover:text-gray-800'"
          @click="tab = 'preview'"
        >
          Preview
        </button>
      </span>
    </div>

    <textarea
      v-show="tab === 'edit'"
      :id="inputId"
      ref="textareaRef"
      :value="modelValue"
      :rows="rows"
      :placeholder="placeholder"
      class="block w-full p-3 font-mono text-sm focus:outline-none"
      @input="emit('update:modelValue', ($event.target as HTMLTextAreaElement).value)"
      @keydown="onKeydown"
    ></textarea>

    <div v-if="tab === 'preview'" class="relative">
      <div v-if="previewLoading" class="px-3 py-1 text-xs text-gray-500">Rendering…</div>
      <p v-if="previewError" class="px-3 py-1 text-xs text-red-600">{{ previewError }}</p>
      <div class="prose max-w-none p-4" v-html="previewHtml"></div>
    </div>

    <FilePicker
      v-if="fileLikePicker"
      :exclude-ids="[]"
      :mime-prefix="filePickerMime"
      @pick="pickFile"
      @close="closePicker"
    />

    <div
      v-if="pickerKind === 'gallery'"
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      @click.self="closePicker"
    >
      <div class="flex max-h-[80vh] w-[min(480px,92vw)] flex-col overflow-hidden rounded-lg bg-white shadow-lg">
        <div class="flex items-center justify-between border-b border-gray-200 px-4 py-3">
          <h2 class="font-medium">Pick a gallery</h2>
          <button class="text-gray-500 hover:text-gray-900" @click="closePicker">×</button>
        </div>
        <div class="overflow-auto p-2">
          <p v-if="galleriesLoading" class="px-2 py-2 text-sm text-gray-500">Loading…</p>
          <p v-else-if="galleryPaths.length === 0" class="px-2 py-2 text-sm text-gray-500">No galleries.</p>
          <ul v-else class="m-0 list-none p-0">
            <li
              v-for="p in galleryPaths"
              :key="p"
              class="cursor-pointer rounded px-2 py-1.5 text-sm hover:bg-gray-100"
              @click="pickGallery(p)"
            >
              {{ p }}
            </li>
          </ul>
        </div>
      </div>
    </div>

    <div
      v-if="pickerKind === 'page'"
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
      @click.self="closePicker"
    >
      <div class="flex w-[min(560px,92vw)] flex-col overflow-visible rounded-lg bg-white shadow-lg">
        <div class="flex items-center justify-between border-b border-gray-200 px-4 py-3">
          <h2 class="font-medium">Pick a page</h2>
          <button class="text-gray-500 hover:text-gray-900" @click="closePicker">×</button>
        </div>
        <div class="p-4">
          <PathPicker v-model="pickerPagePath" namespace="page" placeholder="section/page" />
        </div>
        <div class="flex justify-end gap-2 border-t border-gray-200 px-4 py-2">
          <button
            type="button"
            class="rounded border border-gray-300 px-3 py-1.5 text-sm hover:border-gray-500"
            @click="closePicker"
          >
            Cancel
          </button>
          <button
            type="button"
            class="rounded bg-gray-800 px-3 py-1.5 text-sm text-white hover:bg-gray-700 disabled:opacity-50"
            :disabled="!pickerPagePath.trim()"
            @click="confirmPagePick"
          >
            Insert
          </button>
        </div>
      </div>
    </div>
  </div>
</template>
