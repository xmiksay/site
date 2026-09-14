<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useGalleriesStore } from '../stores/galleries'
import { useFilesStore } from '../stores/files'
import FilePicker from '../components/FilePicker.vue'
import PathPicker from '../components/PathPicker.vue'

const props = defineProps<{ id?: string; create?: boolean }>()

const router = useRouter()
const galleries = useGalleriesStore()
const files = useFilesStore()

const path = ref('')
const title = ref('')
const description = ref('')
const fileIds = ref<number[]>([])
const showPicker = ref(false)
const error = ref('')

onMounted(async () => {
  await files.load()
  if (!props.create && props.id) {
    const g = await galleries.read(Number(props.id))
    path.value = g.path
    title.value = g.title
    description.value = g.description ?? ''
    fileIds.value = [...g.file_ids]
  }
})

async function save() {
  error.value = ''
  try {
    if (props.create) {
      await galleries.create({
        path: path.value,
        title: title.value,
        description: description.value || null,
        file_ids: fileIds.value,
      })
    } else if (props.id) {
      await galleries.update(Number(props.id), {
        path: path.value,
        title: title.value,
        description: description.value || null,
        file_ids: fileIds.value,
      })
    }
    router.push('/galleries')
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'Save failed'
  }
}

function lookup(id: number) {
  return files.items.find((f) => f.id === id)
}

function pickFile(id: number) {
  fileIds.value.push(id)
  showPicker.value = false
}

function removeFile(id: number) {
  fileIds.value = fileIds.value.filter((x) => x !== id)
}

function move(idx: number, dir: -1 | 1) {
  const target = idx + dir
  if (target < 0 || target >= fileIds.value.length) return
  const arr = fileIds.value
  ;[arr[idx], arr[target]] = [arr[target], arr[idx]]
}
</script>

<template>
  <div class="space-y-4">
    <div class="flex items-center justify-between">
      <h1 class="text-xl font-semibold">
        {{ props.create ? 'New gallery' : 'Edit gallery' }}
      </h1>
      <div class="space-x-2 text-sm">
        <router-link to="/galleries" class="text-fg-2 hover:underline">Cancel</router-link>
        <button class="rounded button-primary px-3 py-1.5" @click="save">
          Save
        </button>
      </div>
    </div>
    <p v-if="error" class="text-danger text-sm">{{ error }}</p>

    <div class="bg-surface shadow rounded p-4 space-y-3 max-w-2xl">
      <label class="block">
        <span class="text-sm text-fg-2">Path</span>
        <PathPicker
          v-model="path"
          namespace="all"
          placeholder="holiday-2024"
          class="mt-1"
        />
      </label>
      <label class="block">
        <span class="text-sm text-fg-2">Title</span>
        <input v-model="title" class="mt-1 w-full rounded border border-line-1 px-2 py-1.5" />
      </label>
      <label class="block">
        <span class="text-sm text-fg-2">Description</span>
        <textarea
          v-model="description"
          rows="2"
          class="mt-1 w-full rounded border border-line-1 px-2 py-1.5"
        ></textarea>
      </label>
    </div>

    <div class="bg-surface shadow rounded p-4">
      <div class="flex items-center justify-between mb-2">
        <h2 class="font-medium">Files in this gallery</h2>
        <button
          class="rounded button-primary px-3 py-1.5 text-sm"
          @click="showPicker = true"
        >
          Add file
        </button>
      </div>
      <ul class="divide-y divide-line-3">
        <li
          v-for="(id, idx) in fileIds"
          :key="`${id}-${idx}`"
          class="flex items-center gap-3 py-2"
        >
          <img
            v-if="lookup(id)?.has_thumbnail"
            :src="`/files/${lookup(id)?.hash}/nahled`"
            class="w-12 h-12 object-cover bg-surface-raised rounded"
          />
          <div v-else class="w-12 h-12 bg-surface-raised rounded flex items-center justify-center text-xs text-fg-4">
            ?
          </div>
          <span class="flex-1 text-sm">{{ lookup(id)?.title ?? `File ${id}` }}</span>
          <button class="text-xs text-fg-2 hover:underline" @click="move(idx, -1)" :disabled="idx === 0">
            ↑
          </button>
          <button
            class="text-xs text-fg-2 hover:underline"
            @click="move(idx, 1)"
            :disabled="idx === fileIds.length - 1"
          >
            ↓
          </button>
          <button class="text-xs text-danger hover:underline" @click="removeFile(id)">Remove</button>
        </li>
        <li v-if="fileIds.length === 0" class="py-4 text-fg-4 text-sm text-center">
          No files yet.
        </li>
      </ul>
    </div>

    <FilePicker
      v-if="showPicker"
      :exclude-ids="fileIds"
      @pick="pickFile"
      @close="showPicker = false"
    />
  </div>
</template>
