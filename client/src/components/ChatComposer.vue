<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useAssistantStore } from '../stores/assistant'

const props = defineProps<{
  pageContext?: { id: number; path: string }
  placeholder?: string
}>()

const assistant = useAssistantStore()
const draft = ref('')
const error = ref('')

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

// The draft is cleared up front: the composer stays usable for another chat
// while this turn runs (`sending` is per session), so clearing it afterwards
// could wipe text typed there. A failed send puts the text back.
async function send() {
  const text = draft.value.trim()
  const id = assistant.current?.id
  if (!text || !id) return
  draft.value = ''
  error.value = ''
  try {
    await assistant.sendMessage(id, text)
  } catch (e) {
    if (!draft.value) draft.value = text
    error.value = e instanceof Error ? e.message : String(e)
  }
}

/** Appends `text` to the composer (e.g. a note about an uploaded asset). */
function insert(text: string) {
  draft.value = draft.value.trim() ? `${draft.value.trimEnd()}\n${text}` : text
}
defineExpose({ insert })
</script>

<template>
  <footer v-if="assistant.current" class="shrink-0 p-3 border-t border-line-2">
    <p v-if="error" class="mb-2 text-xs text-danger">{{ error }}</p>
    <form class="flex gap-2" @submit.prevent="send">
      <textarea
        v-model="draft"
        rows="2"
        class="flex-1 border border-line-1 rounded p-2 text-sm resize-none bg-surface"
        :placeholder="hint"
        :disabled="assistant.sending"
        @keydown.meta.enter.prevent="send"
        @keydown.ctrl.enter.prevent="send"
      ></textarea>
      <button
        type="submit"
        class="rounded button-primary px-3 py-2 text-sm disabled:opacity-50"
        :disabled="assistant.sending || draft.trim() === ''"
      >
        Send
      </button>
    </form>
  </footer>
</template>
