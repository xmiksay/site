<script setup lang="ts">
import { onBeforeUnmount, watch } from 'vue'
import { useEditor, EditorContent } from '@tiptap/vue-3'
import StarterKit from '@tiptap/starter-kit'
import { Markdown, type MarkdownStorage } from 'tiptap-markdown'

const props = defineProps<{ modelValue: string }>()
const emit = defineEmits<{ (e: 'update:modelValue', v: string): void }>()

// tiptap-markdown does not augment Tiptap v3's Storage type, so reach the
// serializer through a narrow cast instead of `editor.storage.markdown`.
function getMarkdown(storage: unknown): string {
  return ((storage as { markdown: MarkdownStorage }).markdown).getMarkdown()
}

// Guards onUpdate emits during programmatic setContent so that merely switching
// tabs (which calls setContent) never mutates modelValue — only real typing does.
let applyingExternal = false

const editor = useEditor({
  extensions: [
    StarterKit,
    Markdown.configure({
      html: true,
      transformPastedText: true,
      transformCopiedText: true,
    }),
  ],
  content: props.modelValue,
  onUpdate({ editor }) {
    if (applyingExternal) return
    const md = getMarkdown(editor.storage)
    emit('update:modelValue', md)
  },
})

// Keep WYSIWYG in sync when the raw textarea (or external load) changes the
// markdown. Only reset content when it actually differs to avoid cursor jumps.
watch(
  () => props.modelValue,
  (newVal) => {
    const ed = editor.value
    if (!ed) return
    const current = getMarkdown(ed.storage)
    if (newVal === current) return
    applyingExternal = true
    // Tiptap v3: second arg is an options object; emitUpdate:false suppresses
    // onUpdate, but applyingExternal is the real guarantee modelValue is untouched.
    ed.commands.setContent(newVal, { emitUpdate: false })
    applyingExternal = false
  },
)

onBeforeUnmount(() => {
  editor.value?.destroy()
})
</script>

<template>
  <div class="prose max-w-none p-3 min-h-[24rem] focus:outline-none">
    <EditorContent :editor="editor" />
  </div>
</template>
