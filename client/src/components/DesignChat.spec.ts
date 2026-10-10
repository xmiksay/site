import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import { defineComponent, h } from 'vue'
import DesignChat from './DesignChat.vue'
import { useAssistantStore } from '../stores/assistant'
import type { AssistantSession, AssistantSessionDetail, LlmModel } from '../types'

function session(id: number, agent_profile: string, parent_session_id: number | null = null): AssistantSession {
  return {
    id,
    title: `chat ${id}`,
    provider: 'p',
    model: 'm',
    model_id: 1,
    enabled_mcp_server_ids: [],
    temperature: null,
    reasoning_effort: null,
    max_output_tokens: null,
    thinking_budget_tokens: null,
    agent_profile,
    parent_session_id,
    root_engine_session_id: 'e',
    created_at: '2026-10-10T12:00:00Z',
    updated_at: '2026-10-10T12:00:00Z',
  }
}

function detail(s: AssistantSession): AssistantSessionDetail {
  return { ...s, messages: [] } as unknown as AssistantSessionDetail
}

const stubs = { ChatPanel: true }

function setup(sessions: AssistantSession[]) {
  const assistant = useAssistantStore()
  vi.spyOn(assistant, 'loadSessions').mockImplementation(async () => {
    assistant.sessions = sessions
  })
  vi.spyOn(assistant, 'loadModels').mockImplementation(async () => {
    assistant.models = [{ id: 1 } as LlmModel]
  })
  vi.spyOn(assistant, 'loadMcpServers').mockResolvedValue()
  const loadSession = vi.spyOn(assistant, 'loadSession').mockImplementation(async (id: number) => {
    assistant.current = detail(sessions.find((s) => s.id === id) ?? session(id, 'designer'))
    return assistant.current
  })
  return { assistant, loadSession }
}

describe('DesignChat', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it('offers only root Designer chats and resumes the newest', async () => {
    const { loadSession } = setup([session(3, 'designer'), session(2, 'build'), session(4, 'researcher', 3)])
    const wrapper = mount(DesignChat, { global: { stubs } })
    await flushPromises()

    expect(wrapper.findAll('option').map((o) => o.text())).toEqual([expect.stringContaining('chat 3')])
    expect(loadSession).toHaveBeenCalledWith(3)
  })

  it('does not reopen a non-designer chat left open in the assistant', async () => {
    const { assistant } = setup([session(2, 'build')])
    assistant.current = detail(session(2, 'build'))
    const wrapper = mount(DesignChat, { global: { stubs } })
    await flushPromises()

    expect(assistant.current).toBeNull()
    expect(wrapper.text()).toContain('Start a Designer chat')
  })

  it('"New Designer chat" creates a session under the designer profile', async () => {
    const { assistant, loadSession } = setup([])
    const create = vi
      .spyOn(assistant, 'createSession')
      .mockResolvedValue(session(9, 'designer'))
    const wrapper = mount(DesignChat, { global: { stubs } })
    await flushPromises()

    await wrapper.findAll('button').find((b) => b.text() === 'New Designer chat')!.trigger('click')
    await flushPromises()
    expect(create).toHaveBeenCalledWith({ title: 'Design', agent_profile: 'designer' })
    expect(loadSession).toHaveBeenLastCalledWith(9)
  })

  it('attaches through the composer only — no separate "Attach asset…" action', async () => {
    setup([session(3, 'designer')])
    // Renders the `actions` slot, where the old attach control lived.
    const ChatPanel = defineComponent({
      setup(_, { slots }) {
        return () => h('div', slots.actions?.())
      },
    })
    const wrapper = mount(DesignChat, { global: { stubs: { ChatPanel } } })
    await flushPromises()
    expect(wrapper.find('input[type="file"]').exists()).toBe(false)
    expect(wrapper.text()).not.toContain('Attach asset')
  })
})
