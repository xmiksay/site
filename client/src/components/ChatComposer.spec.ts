import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import { nextTick } from 'vue'
import ChatComposer from './ChatComposer.vue'
import { useAssistantStore } from '../stores/assistant'
import type { AssistantSessionDetail } from '../types'

function open(id: number, messages: unknown[] = [{ id: 1 }]) {
  const assistant = useAssistantStore()
  assistant.current = { id, title: 't', messages } as unknown as AssistantSessionDetail
  return assistant
}

describe('ChatComposer', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it('insert() appends a line to the draft', async () => {
    open(1)
    const wrapper = mount(ChatComposer)
    const textarea = wrapper.find('textarea')
    await textarea.setValue('Make the header blue. ')
    ;(wrapper.vm as unknown as { insert(t: string): void }).insert('I uploaded assets/img/a.png to the draft.')
    await nextTick()
    expect(textarea.element.value).toBe('Make the header blue.\nI uploaded assets/img/a.png to the draft.')
  })

  it('sends the trimmed draft to the open session and clears it up front', async () => {
    const assistant = open(4)
    let finish!: () => void
    const send = vi
      .spyOn(assistant, 'sendMessage')
      .mockReturnValue(new Promise((r) => (finish = () => r({} as AssistantSessionDetail))))
    const wrapper = mount(ChatComposer)
    await wrapper.find('textarea').setValue('  hello  ')
    await wrapper.find('form').trigger('submit')

    expect(send).toHaveBeenCalledWith(4, 'hello')
    expect(wrapper.find('textarea').element.value).toBe('')
    finish()
    await flushPromises()
  })

  it('puts the text back and shows the error when the send fails', async () => {
    const assistant = open(4)
    vi.spyOn(assistant, 'sendMessage').mockRejectedValue(new Error('boom'))
    const wrapper = mount(ChatComposer)
    await wrapper.find('textarea').setValue('hello')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(wrapper.find('textarea').element.value).toBe('hello')
    expect(wrapper.text()).toContain('boom')
  })

  it('pre-fills the page path when an empty page chat opens', async () => {
    const assistant = useAssistantStore()
    const wrapper = mount(ChatComposer, { props: { pageContext: { id: 7, path: 'notes/a' } } })
    assistant.current = { id: 3, title: 't', messages: [] } as unknown as AssistantSessionDetail
    await nextTick()
    expect(wrapper.find('textarea').element.value).toBe('Page /notes/a: ')
    expect(wrapper.find('textarea').attributes('placeholder')).toContain('Ask about this page')
  })

  it('a placeholder prop overrides the default hint', () => {
    open(1)
    const wrapper = mount(ChatComposer, { props: { placeholder: 'Describe the design change…' } })
    expect(wrapper.find('textarea').attributes('placeholder')).toBe('Describe the design change…')
  })

  it("a failed send does not land in another chat's composer", async () => {
    const assistant = open(4)
    let fail!: (e: Error) => void
    vi.spyOn(assistant, 'sendMessage').mockReturnValue(new Promise((_, r) => (fail = r)))
    const wrapper = mount(ChatComposer)
    await wrapper.find('textarea').setValue('for chat 4')
    await wrapper.find('form').trigger('submit')

    open(5)
    await nextTick()
    fail(new Error('boom'))
    await flushPromises()
    expect(wrapper.find('textarea').element.value).toBe('')
    expect(wrapper.text()).not.toContain('boom')
  })

  it('switching chats clears a shown send error', async () => {
    const assistant = open(4)
    vi.spyOn(assistant, 'sendMessage').mockRejectedValue(new Error('boom'))
    const wrapper = mount(ChatComposer)
    await wrapper.find('textarea').setValue('hello')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(wrapper.text()).toContain('boom')

    open(5)
    await nextTick()
    expect(wrapper.text()).not.toContain('boom')
  })
})
