<script setup lang="ts">
// The chat sidebar: the flat session list folded into its spawn tree (#103).
// Split out of `AssistantView.vue` to keep that file under the line cap.
//
// Collapsed by default — a root's per-turn spawn budget is 16, so a
// research-heavy chat contributes dozens of child rows that would otherwise
// bury the roots. Delete is offered on roots only: a child delete is a 409
// server-side (the root's own delete cascades the sub-tree).
import { computed, ref, watch } from 'vue'
import type { AssistantSession } from '../types'
import { ancestorIds, buildSessionTree, flattenSessionTree } from '../composables/useSessionTree'
import { profileIcon } from '../composables/useAssistantContent'

const props = defineProps<{ sessions: AssistantSession[]; currentId: number | null }>()
const emit = defineEmits<{ select: [id: number]; delete: [id: number] }>()

const expanded = ref(new Set<number>())

const tree = computed(() => buildSessionTree(props.sessions))
const rows = computed(() => flattenSessionTree(tree.value, (id) => expanded.value.has(id)))

// Reveal a child opened from somewhere else — a sub-agent card in the
// transcript, or a WS-driven refetch — instead of leaving the selected row
// collapsed out of sight. Watches the list too, since the selection can land
// before the row that explains its ancestry does.
watch(
  () => [props.currentId, props.sessions.length] as const,
  () => {
    if (props.currentId == null) return
    for (const id of ancestorIds(props.sessions, props.currentId)) expanded.value.add(id)
  },
  { immediate: true },
)

function toggle(id: number) {
  if (expanded.value.has(id)) expanded.value.delete(id)
  else expanded.value.add(id)
}

function setExpanded(id: number, open: boolean) {
  if (open) expanded.value.add(id)
  else expanded.value.delete(id)
}

// A row with a parent is a sub-agent even when it renders at root level
// because its parent is missing from the list (an orphan) — so this keys off
// the row itself, never off `depth`, which would offer a 409 delete on it.
function isRoot(session: AssistantSession): boolean {
  return session.parent_session_id == null
}

function subline(session: AssistantSession, childCount: number): string {
  const lead = isRoot(session) ? session.model : session.agent_profile
  return childCount > 0 ? `${lead} · ${childCount} sub-agent${childCount > 1 ? 's' : ''}` : lead
}
</script>

<template>
  <div class="flex-1 overflow-y-auto" role="tree" aria-label="Chats">
    <div
      v-for="node in rows"
      :key="node.session.id"
      role="treeitem"
      :aria-level="node.depth + 1"
      :aria-expanded="node.children.length > 0 ? expanded.has(node.session.id) : undefined"
      :aria-selected="node.session.id === currentId"
      class="flex items-stretch border-b border-line-3"
      :class="node.session.id === currentId ? 'bg-surface-raised' : 'hover:bg-surface-alt'"
    >
      <!-- One guide rule per nesting step: cheap depth cue that stays legible
           when a sub-agent spawns its own sub-agent. -->
      <span
        v-for="level in node.depth"
        :key="level"
        class="w-3 shrink-0 border-r border-line-3"
      ></span>
      <button
        v-if="node.children.length > 0"
        type="button"
        class="w-5 shrink-0 text-xs text-fg-4 hover:text-fg-1"
        :aria-label="
          (expanded.has(node.session.id) ? 'Collapse ' : 'Expand ') + node.session.title
        "
        @click="toggle(node.session.id)"
      >
        {{ expanded.has(node.session.id) ? '▾' : '▸' }}
      </button>
      <span v-else class="w-5 shrink-0"></span>
      <button
        type="button"
        class="flex-1 min-w-0 text-left px-1 py-2"
        :title="
          isRoot(node.session)
            ? node.session.title
            : `${node.session.title} — sub-agent (read-only settings)`
        "
        @click="emit('select', node.session.id)"
        @keydown.right="setExpanded(node.session.id, true)"
        @keydown.left="setExpanded(node.session.id, false)"
      >
        <div class="text-sm truncate" :class="isRoot(node.session) ? 'font-medium' : ''">
          <span v-if="!isRoot(node.session)" class="text-fg-4">{{
            profileIcon(node.session.agent_profile)
          }}</span>
          {{ node.session.title }}
        </div>
        <div class="text-xs text-fg-3 truncate">
          {{ subline(node.session, node.children.length) }}
        </div>
      </button>
      <button
        v-if="isRoot(node.session)"
        type="button"
        class="px-2 text-xs text-fg-4 hover:text-danger shrink-0"
        :aria-label="`Delete ${node.session.title}`"
        title="Delete"
        @click="emit('delete', node.session.id)"
      >
        ×
      </button>
      <span v-else class="w-6 shrink-0"></span>
    </div>
    <div v-if="rows.length === 0" class="p-4 text-sm text-fg-4">No chats yet.</div>
  </div>
</template>
