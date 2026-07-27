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
