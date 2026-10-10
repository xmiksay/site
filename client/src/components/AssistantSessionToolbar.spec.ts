import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import AssistantSessionToolbar from './AssistantSessionToolbar.vue'
import { useAssistantStore } from '../stores/assistant'
import { ApiError } from '../api'

// A sub-agent session (#99) runs under the model/profile its parent spawned it
// with, and the server refuses to switch either or to compact it (#101) — so
// the toolbar must not offer a control that can only 4xx (#103).
vi.mock('../stores/assistant', () => ({ useAssistantStore: vi.fn() }))

const useAssistantStoreMock = vi.mocked(useAssistantStore)

function withSession(overrides: Record<string, any>, store: Record<string, any> = {}) {
  useAssistantStoreMock.mockReturnValue({
    current: {
      id: 2,
      provider: 'anthropic',
      model: 'sonnet',
      model_id: 1,
      agent_profile: 'build',
      parent_session_id: null,
      enabled_mcp_server_ids: [],
      messages: [{ id: 1 }],
      ...overrides,
    },
    models: [],
    mcpServers: [],
    sending: false,
    ...store,
  } as any)
}

describe('AssistantSessionToolbar', () => {
  beforeEach(() => vi.clearAllMocks())

  it('offers model, profile and compact on a root session', () => {
    withSession({})
    const wrapper = mount(AssistantSessionToolbar)

    expect(wrapper.findAll('select')).toHaveLength(2)
    expect(wrapper.text()).toContain('Compact')
  })

  it('offers the MCP and generation pickers on a root session', () => {
    withSession({})
    const wrapper = mount(AssistantSessionToolbar)

    expect(wrapper.text()).toContain('MCP')
    expect(wrapper.text()).toContain('Gen')
  })

  it('goes read-only on a sub-agent session — no model picker, compact, profile, MCP or generation controls', () => {
    withSession({ parent_session_id: 1, agent_profile: 'researcher' })
    const wrapper = mount(AssistantSessionToolbar)

    expect(wrapper.findAll('select')).toHaveLength(0)
    expect(wrapper.text()).not.toContain('Compact')
    // Replaced by a static "what this child runs as" badge.
    expect(wrapper.text()).toContain('🔎 researcher')
    // `require_root` covers the *whole* PATCH endpoint (#101), so the MCP
    // picker and the generation popover 409 on a child exactly like the model
    // and profile pickers do — every one of them calls `updateSession`.
    expect(wrapper.text()).not.toContain('MCP')
    expect(wrapper.text()).not.toContain('Gen')
    // Nothing clickable is left that would write to the server.
    expect(wrapper.findAll('button')).toHaveLength(0)
  })

  it('shows the 409 for a switch to Designer and keeps the current profile', async () => {
    const updateSession = vi
      .fn()
      .mockRejectedValue(new ApiError(409, 'start a new Designer chat'))
    const loadSession = vi.fn()
    withSession({}, { updateSession, loadSession })
    const alert = vi.spyOn(window, 'alert').mockImplementation(() => {})
    const wrapper = mount(AssistantSessionToolbar)
    const profile = wrapper.findAll('select')[1]!

    await profile.setValue('designer')
    await Promise.resolve()

    expect(updateSession).toHaveBeenCalledWith(2, { agent_profile: 'designer' })
    expect(alert).toHaveBeenCalledWith('start a new Designer chat')
    expect((profile.element as HTMLSelectElement).value).toBe('build')
    expect(loadSession).not.toHaveBeenCalled()
    alert.mockRestore()
  })
})
