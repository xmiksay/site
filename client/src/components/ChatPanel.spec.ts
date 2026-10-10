import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import ChatPanel from './ChatPanel.vue'
import { useAssistantStore } from '../stores/assistant'
import type { AssistantSession, AssistantSessionDetail } from '../types'

const stubs = { ChatPanelHeader: true, ChatMessageList: true }

// Node 25 ships its own (here unconfigured) `localStorage` global, which
// shadows jsdom's — so give the panel a plain in-memory one.
function memoryStorage() {
  const items = new Map<string, string>()
  return {
    getItem: (k: string) => items.get(k) ?? null,
    setItem: (k: string, v: string) => void items.set(k, v),
  }
}
const page = { id: 7, path: 'notes/a' }

function setup() {
  const assistant = useAssistantStore()
  vi.spyOn(assistant, 'loadModels').mockResolvedValue()
  vi.spyOn(assistant, 'loadPermissions').mockResolvedValue()
  vi.spyOn(assistant, 'loadMcpServers').mockResolvedValue()
  const loadSession = vi.spyOn(assistant, 'loadSession').mockImplementation(async (id: number) => {
    assistant.current = { id, title: 't', messages: [] } as unknown as AssistantSessionDetail
    return assistant.current
  })
  const createSession = vi
    .spyOn(assistant, 'createSession')
    .mockResolvedValue({ id: 11 } as AssistantSession)
  return { assistant, loadSession, createSession }
}

describe('ChatPanel', () => {
  afterEach(() => vi.unstubAllGlobals())
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.stubGlobal('localStorage', memoryStorage())
  })

  it('leaves the open session alone without a page context', async () => {
    const { loadSession, createSession, assistant } = setup()
    mount(ChatPanel, { global: { stubs } })
    await flushPromises()
    expect(loadSession).not.toHaveBeenCalled()
    expect(createSession).not.toHaveBeenCalled()
    expect(assistant.loadModels).not.toHaveBeenCalled()
  })

  it("resumes the page's saved chat", async () => {
    const { loadSession, createSession } = setup()
    localStorage.setItem('assistant_page_session_7', '5')
    mount(ChatPanel, { props: { pageContext: page }, global: { stubs } })
    await flushPromises()
    expect(loadSession).toHaveBeenCalledWith(5)
    expect(createSession).not.toHaveBeenCalled()
  })

  it('starts a new page chat when the saved one is gone', async () => {
    const { loadSession, createSession } = setup()
    localStorage.setItem('assistant_page_session_7', '5')
    loadSession.mockRejectedValueOnce(new Error('404'))
    mount(ChatPanel, { props: { pageContext: page }, global: { stubs } })
    await flushPromises()
    expect(createSession).toHaveBeenCalledWith({ title: 'Page: notes/a' })
    expect(loadSession).toHaveBeenLastCalledWith(11)
    expect(localStorage.getItem('assistant_page_session_7')).toBe('11')
  })

  it('insert() reaches the composer', async () => {
    const { assistant } = setup()
    assistant.current = { id: 1, title: 't', messages: [{ id: 1 }] } as unknown as AssistantSessionDetail
    const wrapper = mount(ChatPanel, { global: { stubs } })
    ;(wrapper.vm as unknown as { insert(t: string): void }).insert('note')
    await flushPromises()
    expect(wrapper.find('textarea').element.value).toBe('note')
  })
})
