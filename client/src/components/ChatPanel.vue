<script setup lang="ts">
import { onMounted } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import ChatPanelHeader from './ChatPanelHeader.vue'
import ChatComposer from './ChatComposer.vue'
import ChatMessageList from './ChatMessageList.vue'

const props = defineProps<{
  pageContext?: { id: number; path: string }
  showBack?: boolean
}>()

const emit = defineEmits<{ back: [] }>()

const assistant = useAssistantStore()

const pageSessionKey = (id: number) => `assistant_page_session_${id}`

onMounted(async () => {
  await Promise.all([
    assistant.loadModels(),
    assistant.loadPermissions(),
    assistant.loadMcpServers(),
  ])

  if (props.pageContext) {
    const key = pageSessionKey(props.pageContext.id)
    const savedId = localStorage.getItem(key)
    if (savedId) {
      try {
        await assistant.loadSession(Number(savedId))
      } catch {
        await initPageSession()
      }
    } else {
      await initPageSession()
    }
  }
})

async function initPageSession() {
  if (!props.pageContext) return
  const s = await assistant.createSession({ title: `Page: ${props.pageContext.path}` })
  localStorage.setItem(pageSessionKey(props.pageContext.id), String(s.id))
  await assistant.loadSession(s.id)
}

</script>

<template>
  <div class="flex flex-col bg-white rounded-lg shadow overflow-hidden">
    <ChatPanelHeader :show-back="showBack" @back="emit('back')" />
    
    <ChatMessageList />

    <ChatComposer :page-context="pageContext" />

    <div v-if="!assistant.current" class="flex-1 flex items-center justify-center text-sm text-gray-500">
      {{ assistant.models.length === 0 ? 'No models configured.' : 'Loading…' }}
    </div>
  </div>
</template>

<style scoped>
.assistant-markdown :deep(p) {
  margin: 0.25rem 0;
}
.assistant-markdown :deep(p:first-child) {
  margin-top: 0;
}
.assistant-markdown :deep(p:last-child) {
  margin-bottom: 0;
}
.assistant-markdown :deep(h1),
.assistant-markdown :deep(h2),
.assistant-markdown :deep(h3),
.assistant-markdown :deep(h4) {
  font-weight: 600;
  margin: 0.75rem 0 0.25rem;
  line-height: 1.25;
}
.assistant-markdown :deep(h1) { font-size: 1.25rem; }
.assistant-markdown :deep(h2) { font-size: 1.15rem; }
.assistant-markdown :deep(h3) { font-size: 1.05rem; }
.assistant-markdown :deep(ul),
.assistant-markdown :deep(ol) {
  margin: 0.25rem 0;
  padding-left: 1.5rem;
}
.assistant-markdown :deep(ul) { list-style: disc; }
.assistant-markdown :deep(ol) { list-style: decimal; }
.assistant-markdown :deep(li) { margin: 0.125rem 0; }
.assistant-markdown :deep(a) {
  color: #1d4ed8;
  text-decoration: underline;
}
.assistant-markdown :deep(code) {
  background: rgba(0, 0, 0, 0.06);
  padding: 0.05rem 0.3rem;
  border-radius: 0.25rem;
  font-size: 0.875em;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
.assistant-markdown :deep(pre) {
  background: #1f2937;
  color: #f3f4f6;
  padding: 0.75rem;
  border-radius: 0.375rem;
  overflow-x: auto;
  margin: 0.5rem 0;
  font-size: 0.85em;
}
.assistant-markdown :deep(pre code) {
  background: transparent;
  padding: 0;
  color: inherit;
  font-size: inherit;
}
.assistant-markdown :deep(blockquote) {
  border-left: 3px solid #d1d5db;
  padding-left: 0.75rem;
  color: #4b5563;
  margin: 0.5rem 0;
}
.assistant-markdown :deep(hr) {
  border: 0;
  border-top: 1px solid #e5e7eb;
  margin: 0.75rem 0;
}
.assistant-markdown :deep(table) {
  border-collapse: collapse;
  margin: 0.5rem 0;
}
.assistant-markdown :deep(th),
.assistant-markdown :deep(td) {
  border: 1px solid #e5e7eb;
  padding: 0.25rem 0.5rem;
  text-align: left;
}
.assistant-markdown :deep(th) {
  background: #f9fafb;
  font-weight: 600;
}
</style>
