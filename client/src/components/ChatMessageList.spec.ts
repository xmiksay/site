import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import { defineComponent, h, nextTick } from 'vue'
import ChatMessageList from './ChatMessageList.vue'
import { useAssistantStore } from '../stores/assistant'
import type { AssistantSessionDetail } from '../types'

// Stands in for a transcript message whose sub-agent card was clicked.
const AssistantMessageContent = defineComponent({
  emits: ['select-session'],
  setup(_, { emit }) {
    return () => h('button', { class: 'card', onClick: () => emit('select-session', 42) })
  },
})

function open(id: number) {
  const assistant = useAssistantStore()
  assistant.current = { id, title: 't', messages: [{ id: 1, role: 'assistant', content: {} }] } as unknown as AssistantSessionDetail
  return assistant
}

describe('ChatMessageList', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it("shows the error when a sub-agent's session cannot be opened", async () => {
    const assistant = open(1)
    vi.spyOn(assistant, 'loadSession').mockRejectedValue(new Error('session not found'))
    const wrapper = mount(ChatMessageList, { global: { stubs: { AssistantMessageContent } } })

    await wrapper.find('button.card').trigger('click')
    await flushPromises()
    expect(assistant.loadSession).toHaveBeenCalledWith(42)
    expect(wrapper.text()).toContain('session not found')

    open(2)
    await nextTick()
    expect(wrapper.text()).not.toContain('session not found')
  })

  it('drops a slow failure that lands after a chat switch', async () => {
    const assistant = open(1)
    let fail!: (e: Error) => void
    vi.spyOn(assistant, 'loadSession').mockReturnValue(new Promise((_, r) => (fail = r)))
    const wrapper = mount(ChatMessageList, { global: { stubs: { AssistantMessageContent } } })

    await wrapper.find('button.card').trigger('click')
    open(2)
    await nextTick()
    fail(new Error('session not found'))
    await flushPromises()
    expect(wrapper.text()).not.toContain('session not found')
  })
})
