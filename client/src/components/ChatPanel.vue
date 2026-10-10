<script setup lang="ts">
// The open session's header, transcript and composer — shared by
// `AssistantView`, the page editor and the Design studio. The parent owns
// the card styling and which session is open; with `pageContext` the panel
// opens (or creates) that page's own chat itself.
import { onMounted, ref } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import ChatPanelHeader from './ChatPanelHeader.vue'
import ChatComposer from './ChatComposer.vue'
import ChatMessageList from './ChatMessageList.vue'

const props = defineProps<{
  pageContext?: { id: number; path: string }
  showBack?: boolean
  placeholder?: string
}>()

const emit = defineEmits<{ back: [] }>()

const assistant = useAssistantStore()
const composer = ref<InstanceType<typeof ChatComposer> | null>(null)
const pageSessionKey = (id: number) => `assistant_page_session_${id}`

/** Appends `text` to the composer (e.g. a note about an uploaded asset). */
function insert(text: string) {
  composer.value?.insert(text)
}
defineExpose({ insert })

// The assistant view and the studio load these themselves; the page editor
// has nobody else to do it.
onMounted(async () => {
  if (!props.pageContext) return
  await Promise.all([assistant.loadModels(), assistant.loadPermissions(), assistant.loadMcpServers()])
  await loadOrInitPageSession(props.pageContext)
})

async function loadOrInitPageSession(page: { id: number; path: string }) {
  const savedId = localStorage.getItem(pageSessionKey(page.id))
  if (savedId) {
    try {
      await assistant.loadSession(Number(savedId))
      return
    } catch {
      // The saved chat was deleted — start a fresh one below.
    }
  }
  const s = await assistant.createSession({ title: `Page: ${page.path}` })
  localStorage.setItem(pageSessionKey(page.id), String(s.id))
  await assistant.loadSession(s.id)
}
</script>

<template>
  <div class="flex flex-col min-h-0">
    <ChatPanelHeader :show-back="showBack" @back="emit('back')">
      <slot name="actions" />
    </ChatPanelHeader>

    <ChatMessageList />

    <ChatComposer ref="composer" :page-context="pageContext" :placeholder="placeholder" />

    <div v-if="!assistant.current" class="flex-1 flex items-center justify-center text-sm text-fg-3">
      {{ assistant.models.length === 0 ? 'No models configured.' : 'Loading…' }}
    </div>
  </div>
</template>
