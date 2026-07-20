<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useAssistantStore } from '../stores/assistant'
import ChatMessage from './ChatMessage.vue'


const assistant = useAssistantStore()
const messageList = computed(() => assistant.current?.messages ?? [])
const messageBox = ref<HTMLDivElement | null>(null)


function scrollToBottom() {
  nextTick(() => {
    if (messageBox.value) {
      messageBox.value.scrollTop = messageBox.value.scrollHeight
    }
  })
}

watch(() => assistant.current, scrollToBottom)


</script>


<template>
  <div ref="messageBox" class="flex-1 overflow-y-auto p-4 space-y-3 min-h-0">
    <ChatMessage v-for="m in messageList" :key="m.id" :message="m" />
    
    <div v-if="assistant.sending" class="text-xs text-gray-500">thinking…</div>
  </div>
</template>