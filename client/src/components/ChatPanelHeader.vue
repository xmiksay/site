<script setup lang="ts">
import { useAssistantStore } from '../stores/assistant'
import AssistantSessionToolbar from './AssistantSessionToolbar.vue'

defineProps<{
  showBack?: boolean
}>()
const emit = defineEmits<{ back: [] }>()

const assistant = useAssistantStore()

async function promptToChangeTitle() {
  const current = assistant.current
  if (!current) return
  const newTitle = prompt('New title', current.title)
  if (!newTitle || newTitle === current.title) return

  await assistant.updateTitle(newTitle)
}
</script>

<template>
  <header
    v-if="assistant.current"
    class="shrink-0 p-3 border-b border-line-2 flex flex-wrap items-center justify-between gap-2"
  >
    <div class="flex items-center gap-2 min-w-0">
      <button
        v-if="showBack"
        type="button"
        class="md:hidden p-1 rounded hover:bg-surface-raised text-fg-2 shrink-0"
        aria-label="Back to chats"
        @click="emit('back')"
      >
        <svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7" />
        </svg>
      </button>
      <button
        class="text-left hover:underline truncate font-semibold text-sm"
        @click="promptToChangeTitle"
        :title="assistant.current.title"
      >
        {{ assistant.current.title }}
      </button>
    </div>
    <div class="flex flex-wrap items-center gap-2">
      <AssistantSessionToolbar />
      <slot />
    </div>
  </header>
</template>
