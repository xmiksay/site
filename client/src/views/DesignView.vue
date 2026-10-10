<script setup lang="ts">
// The Design studio (#119): a Designer chat, the draft's files and a live
// draft preview side by side (tabs on narrow screens), with publish /
// discard / history on top.
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { onBeforeRouteLeave } from 'vue-router'
import { useDesignStore } from '../stores/design'
import DesignChat from '../components/DesignChat.vue'
import DesignFilesPanel from '../components/DesignFilesPanel.vue'
import DesignPreview from '../components/DesignPreview.vue'
import DesignStudioToolbar from '../components/DesignStudioToolbar.vue'

type Tab = 'chat' | 'files' | 'preview'
const tabs: { id: Tab; label: string }[] = [
  { id: 'chat', label: 'Chat' },
  { id: 'files', label: 'Files' },
  { id: 'preview', label: 'Preview' },
]

const design = useDesignStore()
const filesPanel = ref<InstanceType<typeof DesignFilesPanel> | null>(null)
const dirty = ref(false)
const error = ref('')
const tab = ref<Tab>('chat')

// Hidden panels stay mounted, so switching tabs keeps the chat, the editor
// and the preview as they were.
function panelClass(id: Tab): string {
  return tab.value === id ? 'flex' : 'hidden xl:flex'
}

function warnUnload(e: BeforeUnloadEvent) {
  if (dirty.value) e.preventDefault()
}

onBeforeRouteLeave(() => filesPanel.value?.confirmDiscard() ?? true)
onMounted(async () => {
  window.addEventListener('beforeunload', warnUnload)
  try {
    await design.load()
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  }
})
onBeforeUnmount(() => window.removeEventListener('beforeunload', warnUnload))
</script>

<template>
  <div class="flex flex-col gap-3 h-[calc(100vh-8rem)] md:h-[calc(100vh-3rem)]">
    <div class="flex flex-wrap items-center gap-3">
      <h1 class="text-xl font-semibold">Design studio</h1>
      <span v-if="design.state" class="text-xs rounded px-2 py-0.5 bg-gray-200 text-gray-700">
        storage: {{ design.state.storage }}
      </span>
    </div>

    <DesignStudioToolbar v-if="design.state" :dirty="dirty" @reset="filesPanel?.reset()" />

    <p v-if="error" class="text-sm text-red-700 bg-red-50 border border-red-200 rounded p-2 whitespace-pre-wrap">
      {{ error }}
    </p>
    <p
      v-if="design.state?.local_dir"
      class="text-sm text-blue-800 bg-blue-50 border border-blue-200 rounded p-2"
    >
      DESIGN_DIR is set: the live site serves files from that local folder first. The preview shows the draft
      regardless.
    </p>

    <template v-if="design.state">
      <nav class="xl:hidden flex gap-1 border-b border-gray-300" role="tablist">
        <button
          v-for="t in tabs"
          :key="t.id"
          type="button"
          role="tab"
          :aria-selected="tab === t.id"
          class="px-3 py-1.5 text-sm -mb-px border-b-2"
          :class="tab === t.id ? 'border-gray-800 font-semibold' : 'border-transparent text-gray-500'"
          @click="tab = t.id"
        >
          {{ t.label }}
        </button>
      </nav>
      <div
        class="flex-1 min-h-0 grid gap-3 xl:grid-cols-[minmax(18rem,22rem)_minmax(0,1fr)_minmax(0,1fr)]"
      >
        <section class="bg-white rounded-lg shadow flex-col min-h-0" :class="panelClass('chat')">
          <DesignChat />
        </section>
        <section class="flex-col min-h-0" :class="panelClass('files')">
          <DesignFilesPanel ref="filesPanel" @dirty="dirty = $event" />
        </section>
        <section class="bg-white rounded-lg shadow flex-col min-h-0 overflow-hidden" :class="panelClass('preview')">
          <DesignPreview />
        </section>
      </div>
    </template>
  </div>
</template>
