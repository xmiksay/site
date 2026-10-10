<script setup lang="ts">
// The studio's draft actions: publish (with the server's 409/422 answers
// turned into a confirm or a readable error list), discard, history and the
// bucket reload. Button labels are Czech by product decision (#119).
import { computed, ref } from 'vue'
import { useDesignStore } from '../stores/design'
import type { PublishOutcome } from '../lib/designPublish'
import DesignHistory from './DesignHistory.vue'

// `dirty`: the open editor has unsaved changes, which are not in the draft.
const props = defineProps<{ dirty: boolean }>()
// The draft was replaced wholesale (discard / restore): reset the editor.
const emit = defineEmits<{ reset: [] }>()

const design = useDesignStore()
const busy = ref(false)
const error = ref('')
const notice = ref('')
const invalid = ref<string[]>([])
const showHistory = ref(false)

const changeCount = computed(() => design.state?.changes.length ?? 0)
const lastReload = computed(() => design.state?.last_reload ?? null)

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

async function run(action: () => Promise<void>) {
  busy.value = true
  error.value = ''
  notice.value = ''
  invalid.value = []
  try {
    await action()
  } catch (e) {
    error.value = message(e)
  } finally {
    busy.value = false
  }
}

function report(outcome: PublishOutcome) {
  switch (outcome.kind) {
    case 'published': {
      const at = new Date(outcome.entry.at).toLocaleString()
      notice.value = `Published — version ${at} (${outcome.entry.files} files) is live.`
      break
    }
    case 'invalid':
      invalid.value = outcome.errors
      break
    case 'nothing':
      notice.value = 'Nothing to publish: the draft matches the live design.'
      break
    case 'conflict':
      error.value = outcome.message
      break
  }
}

function publish() {
  if (props.dirty && !confirm('The open file has unsaved changes, which are not in the draft. Publish anyway?')) {
    return
  }
  return run(async () => {
    let outcome = await design.publish()
    if (outcome.kind === 'conflict') {
      const list = outcome.paths.length ? outcome.paths.map((p) => `• ${p}`).join('\n') : outcome.message
      const overwrite = confirm(
        `The live design changed outside the draft since it was started:\n${list}\n\nPublish anyway and overwrite those changes?`,
      )
      if (!overwrite) return
      outcome = await design.publish(true)
    }
    report(outcome)
  })
}

function discard() {
  const unsaved = props.dirty ? " The open file's unsaved edits are lost too." : ''
  if (!confirm(`Zahodit draft? Every unpublished change is lost and the draft resets to the live design.${unsaved}`)) {
    return
  }
  return run(async () => {
    await design.discard()
    emit('reset')
    notice.value = 'Draft discarded.'
  })
}

function reload() {
  return run(async () => {
    await design.reload()
    notice.value = 'Reloaded the live design from storage.'
  })
}

function onRestored(id: string) {
  showHistory.value = false
  emit('reset')
  error.value = ''
  invalid.value = []
  notice.value = `Version ${id} restored into the draft — review it in the preview, then publish.`
}

/** `templates/x.html:3: message` → the location and the message apart. */
function splitError(line: string): [string, string] {
  const at = line.indexOf(': ')
  return at === -1 ? ['', line] : [line.slice(0, at), line.slice(at + 2)]
}
</script>

<template>
  <div class="space-y-2">
    <div class="flex flex-wrap items-center gap-2 text-sm">
      <span v-if="changeCount" class="text-xs rounded px-2 py-0.5 bg-amber-100 text-amber-800">Draft — not live</span>
      <span v-else class="text-xs rounded px-2 py-0.5 bg-gray-200 text-gray-700">Draft matches live</span>
      <span class="text-gray-600" data-test="change-count">{{ changeCount }} unpublished change(s)</span>
      <button
        :disabled="busy"
        class="rounded bg-gray-800 hover:bg-gray-700 text-white px-3 py-1.5 disabled:opacity-50"
        @click="publish"
      >
        {{ busy ? 'Pracuji…' : 'Publikovat' }}
      </button>
      <button
        :disabled="busy || changeCount === 0"
        class="rounded border border-gray-300 px-3 py-1.5 hover:bg-gray-50 disabled:opacity-50"
        @click="discard"
      >
        Zahodit draft
      </button>
      <button
        class="rounded border border-gray-300 px-3 py-1.5 hover:bg-gray-50"
        :class="{ 'bg-gray-100': showHistory }"
        @click="showHistory = !showHistory"
      >
        Historie
      </button>
      <button
        :disabled="busy"
        class="ml-auto rounded border border-gray-300 px-2 py-1 text-xs hover:bg-gray-50 disabled:opacity-50"
        title="Reload design/ from storage after editing it directly in the bucket"
        @click="reload"
      >
        Reload
      </button>
      <span v-if="lastReload && !lastReload.ok" class="text-xs text-red-600">
        Last reload failed — {{ lastReload.error }}
      </span>
    </div>

    <p v-if="notice" class="text-sm text-green-800 bg-green-50 border border-green-200 rounded p-2">{{ notice }}</p>
    <p v-if="error" class="text-sm text-red-700 bg-red-50 border border-red-200 rounded p-2 whitespace-pre-wrap">
      {{ error }}
    </p>
    <div v-if="invalid.length" class="text-sm text-red-800 bg-red-50 border border-red-200 rounded p-2">
      <p class="font-semibold">Not published — the design failed the render check:</p>
      <ul class="mt-1 space-y-0.5 font-mono text-xs">
        <li v-for="(line, i) in invalid" :key="i">
          <span class="font-semibold">{{ splitError(line)[0] }}</span>
          <template v-if="splitError(line)[0]">: </template>{{ splitError(line)[1] }}
        </li>
      </ul>
    </div>

    <DesignHistory v-if="showHistory" :dirty="dirty" @restored="onRestored" @close="showHistory = false" />
  </div>
</template>
