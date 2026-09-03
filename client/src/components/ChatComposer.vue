<script setup lang="ts">
import { ref, watch } from 'vue'
import { useAssistantStore } from '../stores/assistant'

const props = defineProps<{
  pageContext?: { id: number; path: string }
}>()

const assistant = useAssistantStore()
const draft = ref('')

// Pre-fill draft with page path context when a brand-new (empty) page session opens
watch(
  () => assistant.current,
  (current) => {
    if (props.pageContext && current?.messages.length === 0) {
      draft.value = `Page /${props.pageContext.path}: `
    }
  },
)

async function send() {
  const text = draft.value.trim()
  const id = assistant.current?.id
  if (!text || !id) return

  await assistant.sendMessage(id, text)
  draft.value = ''
}

</script>

<template>

  <footer v-if="assistant.current" class="shrink-0 p-3 border-t">
    <form class="flex gap-2" @submit.prevent="send">
      <textarea
        v-model="draft"
        rows="2"
        class="flex-1 border rounded p-2 text-sm resize-none"
        :placeholder="pageContext ? 'Ask about this page… (Cmd+Enter to send)' : 'Type a message… (Cmd+Enter to send)'"
        :disabled="assistant.sending"
        @keydown.meta.enter.prevent="send"
        @keydown.ctrl.enter.prevent="send"
      ></textarea>
      <button
        type="submit"
        class="rounded bg-gray-800 hover:bg-gray-700 text-white px-3 py-2 text-sm disabled:opacity-50"
        :disabled="assistant.sending || draft.trim() === ''"
      >
        Send
      </button>
    </form>
  </footer>

</template>