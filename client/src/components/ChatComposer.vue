<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import { attachmentNote, DESIGNER_ACCEPT, MAX_ATTACHMENT_BYTES, type ChatAttachment } from '../lib/chatAttachments'
import { formatBytes } from '../lib/format'
import PaperclipIcon from './icons/PaperclipIcon.vue'

const props = defineProps<{
  pageContext?: { id: number; path: string }
  placeholder?: string
}>()

const assistant = useAssistantStore()
const draft = ref('')
const error = ref('')
const uploading = ref(false)

/** A file waiting to go out with the next message; `stored` once uploaded,
 *  so a retry after a failed send does not store it twice. */
interface Pending {
  key: number
  file: File
  stored?: ChatAttachment
}
const pending = ref<Pending[]>([])
let nextKey = 0

const isDesigner = computed(() => assistant.current?.agent_profile === 'designer')
const busy = computed(() => assistant.sending || uploading.value)
const canSend = computed(() => !busy.value && (draft.value.trim() !== '' || pending.value.length > 0))

const hint = computed(
  () =>
    props.placeholder ??
    (props.pageContext ? 'Ask about this page… (Cmd+Enter to send)' : 'Type a message… (Cmd+Enter to send)'),
)

// Pre-fill draft with page path context when a brand-new (empty) page session opens
watch(
  () => assistant.current,
  (current) => {
    if (props.pageContext && current?.messages.length === 0) {
      draft.value = `Page /${props.pageContext.path}: `
    }
  },
)

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/** Queues files for the next message (attach button, drop, paste). */
function addFiles(files: Iterable<File>) {
  const tooLarge: string[] = []
  for (const file of files) {
    if (file.size > MAX_ATTACHMENT_BYTES) tooLarge.push(file.name)
    else pending.value.push({ key: nextKey++, file })
  }
  error.value = tooLarge.length
    ? `Over the ${formatBytes(MAX_ATTACHMENT_BYTES)} limit: ${tooLarge.join(', ')}`
    : ''
}

function remove(key: number) {
  pending.value = pending.value.filter((p) => p.key !== key)
}

function onPick(e: Event) {
  const input = e.target as HTMLInputElement
  addFiles(Array.from(input.files ?? []))
  input.value = ''
}

// Only a paste that carries files is ours; plain text pastes as usual.
function onPaste(e: ClipboardEvent) {
  const files = Array.from(e.clipboardData?.files ?? [])
  if (files.length === 0) return
  e.preventDefault()
  addFiles(files)
}

// Attachments are uploaded before the message: a failed upload sends
// nothing and keeps text and chips. The draft is cleared only once the
// message goes out: the composer stays usable for another chat while this
// turn runs (`sending` is per session), so a failed send puts the text back
// — but only while its own chat is still open, since the composer is shared.
async function send() {
  const text = draft.value.trim()
  const id = assistant.current?.id
  if (!canSend.value || !id) return
  error.value = ''
  const items = pending.value
  if (items.length > 0) {
    uploading.value = true
    try {
      for (const item of items) {
        item.stored ??= await assistant.uploadAttachment(id, item.file)
      }
    } catch (e) {
      if (assistant.current?.id === id) error.value = `Upload failed: ${message(e)}`
      return
    } finally {
      uploading.value = false
    }
  }
  const note = attachmentNote(items.map((p) => p.stored!))
  const full = [text, note].filter(Boolean).join('\n\n')
  if (assistant.current?.id === id) {
    draft.value = ''
    pending.value = []
  }
  try {
    await assistant.sendMessage(id, full)
  } catch (e) {
    if (assistant.current?.id !== id) return
    if (!draft.value) draft.value = text
    if (pending.value.length === 0) pending.value = items
    error.value = message(e)
  }
}

// Attachments belong to the chat they were picked in: a Designer chat
// stores them in the draft, any other as site files.
watch(
  () => assistant.current?.id,
  () => {
    error.value = ''
    pending.value = []
  },
)

defineExpose({ addFiles })
</script>

<template>
  <footer v-if="assistant.current" class="shrink-0 p-3 border-t border-line-2">
    <p v-if="error" class="mb-2 text-xs text-danger">{{ error }}</p>
    <ul v-if="pending.length" class="mb-2 flex flex-wrap gap-1" aria-label="Attachments">
      <li
        v-for="p in pending"
        :key="p.key"
        class="inline-flex items-center gap-1 rounded-full border border-line-1 bg-surface-raised px-2 py-0.5 text-xs text-fg-2"
      >
        <span class="max-w-48 truncate" :title="p.file.name">{{ p.file.name }}</span>
        <span class="text-fg-4">{{ formatBytes(p.file.size) }}</span>
        <button
          type="button"
          class="text-fg-3 hover:text-danger disabled:opacity-50"
          :aria-label="`Remove ${p.file.name}`"
          :disabled="uploading"
          @click="remove(p.key)"
        >
          ×
        </button>
      </li>
    </ul>
    <form class="flex gap-2" @submit.prevent="send">
      <label
        class="self-end rounded border border-line-1 px-2 py-2 text-sm text-fg-2 hover:bg-surface-raised cursor-pointer"
        :class="{ 'opacity-50 pointer-events-none': busy }"
        :title="isDesigner ? 'Attach images or fonts (they go into the draft)' : 'Attach files (up to 10 MB each)'"
      >
        <PaperclipIcon />
        <span class="sr-only">Attach files</span>
        <input
          type="file"
          multiple
          class="hidden"
          :accept="isDesigner ? DESIGNER_ACCEPT : undefined"
          :disabled="busy"
          @change="onPick"
        />
      </label>
      <textarea
        v-model="draft"
        rows="2"
        class="flex-1 border border-line-1 rounded p-2 text-sm resize-none bg-surface"
        :placeholder="hint"
        :disabled="busy"
        @paste="onPaste"
        @keydown.meta.enter.prevent="send"
        @keydown.ctrl.enter.prevent="send"
      ></textarea>
      <button
        type="submit"
        class="rounded button-primary px-3 py-2 text-sm disabled:opacity-50"
        :disabled="!canSend"
      >
        {{ uploading ? 'Uploading…' : 'Send' }}
      </button>
    </form>
  </footer>
</template>
