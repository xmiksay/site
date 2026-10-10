import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount, type VueWrapper } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import ChatComposer from './ChatComposer.vue'
import { useAssistantStore } from '../stores/assistant'
import type { AssistantSessionDetail } from '../types'
import type { ChatAttachment } from '../lib/chatAttachments'

function open(id: number, agent_profile = 'build') {
  const assistant = useAssistantStore()
  assistant.current = { id, title: 't', agent_profile, messages: [{ id: 1 }] } as unknown as AssistantSessionDetail
  return assistant
}

function png(name: string, size = 3): File {
  return new File([new Uint8Array(size)], name, { type: 'image/png' })
}

function stored(path: string, target: ChatAttachment['target'] = 'file'): ChatAttachment {
  return { path, mimetype: 'image/png', size: 3, target }
}

function chips(wrapper: VueWrapper): string[] {
  return wrapper.findAll('li').map((li) => li.find('span').text())
}

async function pick(wrapper: VueWrapper, files: File[]) {
  const input = wrapper.find('input[type="file"]')
  Object.defineProperty(input.element, 'files', { value: files, configurable: true })
  await input.trigger('change')
}

describe('ChatComposer attachments', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it('shows picked files as removable chips', async () => {
    open(1)
    const wrapper = mount(ChatComposer)
    await pick(wrapper, [png('a.png'), png('b.png')])
    expect(chips(wrapper)).toEqual(['a.png', 'b.png'])

    await wrapper.get('button[aria-label="Remove a.png"]').trigger('click')
    expect(chips(wrapper)).toEqual(['b.png'])
  })

  it('refuses files over 10 MB with a message', async () => {
    open(1)
    const wrapper = mount(ChatComposer)
    await pick(wrapper, [png('big.png', 10 * 1024 * 1024 + 1), png('ok.png')])
    expect(chips(wrapper)).toEqual(['ok.png'])
    expect(wrapper.text()).toContain('big.png')
  })

  it('takes files pasted into the textarea, and addFiles() from a drop', async () => {
    open(1)
    const wrapper = mount(ChatComposer)
    const paste = new Event('paste', { cancelable: true }) as ClipboardEvent
    Object.defineProperty(paste, 'clipboardData', { value: { files: [png('image.png')] } })
    wrapper.find('textarea').element.dispatchEvent(paste)
    expect(paste.defaultPrevented).toBe(true)

    ;(wrapper.vm as unknown as { addFiles(f: File[]): void }).addFiles([png('dropped.png')])
    await flushPromises()
    expect(chips(wrapper)).toEqual(['image.png', 'dropped.png'])
  })

  it('a text-only paste is left to the textarea', () => {
    open(1)
    const wrapper = mount(ChatComposer)
    const paste = new Event('paste', { cancelable: true }) as ClipboardEvent
    Object.defineProperty(paste, 'clipboardData', { value: { files: [] } })
    wrapper.find('textarea').element.dispatchEvent(paste)
    expect(paste.defaultPrevented).toBe(false)
  })

  it('uploads first, then sends the text with an attachment note', async () => {
    const assistant = open(4)
    const upload = vi
      .spyOn(assistant, 'uploadAttachment')
      .mockImplementation(async (_id, f) => stored(`uploads/chat/2026-10/${f.name}`))
    const send = vi.spyOn(assistant, 'sendMessage').mockResolvedValue({} as AssistantSessionDetail)
    const wrapper = mount(ChatComposer)
    await pick(wrapper, [png('a.png'), png('b.png')])
    await wrapper.find('textarea').setValue('What is this?')
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(upload.mock.calls.map(([id, f]) => [id, f.name])).toEqual([
      [4, 'a.png'],
      [4, 'b.png'],
    ])
    const [id, text] = send.mock.calls[0]
    expect(id).toBe(4)
    expect(text).toMatch(/^What is this\?\n\nAttached files \(.*file_read.*\):\n/)
    expect(text).toContain('- uploads/chat/2026-10/a.png\n- uploads/chat/2026-10/b.png')
    expect(chips(wrapper)).toEqual([])
    expect(wrapper.find('textarea').element.value).toBe('')
  })

  it('attachments alone are enough to send', async () => {
    const assistant = open(4)
    vi.spyOn(assistant, 'uploadAttachment').mockResolvedValue(stored('uploads/chat/2026-10/a.png'))
    const send = vi.spyOn(assistant, 'sendMessage').mockResolvedValue({} as AssistantSessionDetail)
    const wrapper = mount(ChatComposer)
    expect(wrapper.find('button[type="submit"]').attributes('disabled')).toBeDefined()
    await pick(wrapper, [png('a.png')])
    expect(wrapper.find('button[type="submit"]').attributes('disabled')).toBeUndefined()
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(send.mock.calls[0][1]).toMatch(/^Attached files/)
  })

  it('a failed upload sends nothing and keeps the text and chips', async () => {
    const assistant = open(4)
    vi.spyOn(assistant, 'uploadAttachment').mockRejectedValue(new Error('attachment too large'))
    const send = vi.spyOn(assistant, 'sendMessage')
    const wrapper = mount(ChatComposer)
    await pick(wrapper, [png('a.png')])
    await wrapper.find('textarea').setValue('hello')
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(send).not.toHaveBeenCalled()
    expect(wrapper.find('textarea').element.value).toBe('hello')
    expect(chips(wrapper)).toEqual(['a.png'])
    expect(wrapper.text()).toContain('Upload failed: attachment too large')
  })

  it('a retry after a failed send does not upload again', async () => {
    const assistant = open(4)
    const upload = vi.spyOn(assistant, 'uploadAttachment').mockResolvedValue(stored('uploads/chat/2026-10/a.png'))
    const send = vi
      .spyOn(assistant, 'sendMessage')
      .mockRejectedValueOnce(new Error('boom'))
      .mockResolvedValue({} as AssistantSessionDetail)
    const wrapper = mount(ChatComposer)
    await pick(wrapper, [png('a.png')])
    await wrapper.find('textarea').setValue('hello')
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(chips(wrapper)).toEqual(['a.png'])
    expect(wrapper.find('textarea').element.value).toBe('hello')

    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(upload).toHaveBeenCalledTimes(1)
    expect(send).toHaveBeenCalledTimes(2)
  })

  it('a Designer chat asks for images and fonts and notes design_read', async () => {
    const assistant = open(4, 'designer')
    vi.spyOn(assistant, 'uploadAttachment').mockResolvedValue(stored('assets/img/logo.png', 'design'))
    const send = vi.spyOn(assistant, 'sendMessage').mockResolvedValue({} as AssistantSessionDetail)
    const wrapper = mount(ChatComposer)
    expect(wrapper.find('input[type="file"]').attributes('accept')).toContain('.woff2')
    await pick(wrapper, [png('logo.png')])
    await wrapper.find('form').trigger('submit')
    await flushPromises()
    expect(send.mock.calls[0][1]).toContain('design_read')
    expect(send.mock.calls[0][1]).toContain('- assets/img/logo.png')
  })

  it('switching chats drops pending attachments', async () => {
    open(4)
    const wrapper = mount(ChatComposer)
    await pick(wrapper, [png('a.png')])
    open(5)
    await flushPromises()
    expect(chips(wrapper)).toEqual([])
  })
})
