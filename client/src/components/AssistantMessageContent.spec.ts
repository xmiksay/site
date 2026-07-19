import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import AssistantMessageContent from './AssistantMessageContent.vue'
import { useAssistantStore } from '../stores/assistant'

// Regression coverage for the "historic tool prompt looks unresolved, but
// it's solved in the background" bug: this persisted/REST message view used
// to gate every button on the message-level `requires_approval` flag plus
// "no decision recorded yet", which can't distinguish a genuinely-pending
// call from an auto-allowed sibling or an orphaned decision (see
// `useAssistantContent.spec.ts` and `src/ai/projection/mod.rs`'s
// `mark_resolved_calls` doc for the two root causes). Now it's keyed off each
// call's own `requires_approval`/`resolved` fields instead.
vi.mock('../stores/assistant', () => ({
  useAssistantStore: vi.fn(),
}))

const useAssistantStoreMock = vi.mocked(useAssistantStore)

describe('AssistantMessageContent', () => {
  let approveToolCalls: ReturnType<typeof vi.fn>

  beforeEach(() => {
    approveToolCalls = vi.fn().mockResolvedValue(undefined)
    useAssistantStoreMock.mockReturnValue({
      current: { id: 1 },
      approveToolCalls,
    } as any)
  })

  it('only prompts for the call still gated, not an auto-allowed sibling in the same batch', () => {
    const content = {
      text: null,
      requires_approval: true,
      tool_calls: [
        { id: 'gated', name: 'edit_page', args: { path: 'x' }, requires_approval: true },
        { id: 'auto', name: 'search_pages', args: { q: 'x' } },
      ],
    }
    const wrapper = mount(AssistantMessageContent, {
      props: { role: 'assistant', content, messageId: 0 },
    })

    // Only one call needed a decision, so no "Approve all"/batch row either.
    expect(wrapper.text()).not.toContain('Approve all')
    const buttons = wrapper.findAll('button')
    expect(buttons.map((b) => b.text())).toEqual(['Approve', 'Always allow', 'Reject', 'Always reject'])
  })

  it('does not prompt for a call that already resolved, even with no decisions entry', () => {
    const content = {
      text: null,
      requires_approval: true,
      tool_calls: [{ id: 'a', name: 'edit_page', args: {}, requires_approval: true, resolved: true }],
    }
    const wrapper = mount(AssistantMessageContent, {
      props: { role: 'assistant', content, messageId: 0 },
    })

    expect(wrapper.find('button').exists()).toBe(false)
  })

  it('still prompts for a genuinely pending call with no decisions entry yet', async () => {
    const content = {
      text: null,
      requires_approval: true,
      tool_calls: [{ id: 'a', name: 'edit_page', args: {}, requires_approval: true }],
    }
    const wrapper = mount(AssistantMessageContent, {
      props: { role: 'assistant', content, messageId: 0 },
    })

    await wrapper.find('button').trigger('click')
    expect(approveToolCalls).toHaveBeenCalledWith(1, 0, [{ tool_call_id: 'a', approve: true, remember: false }])
  })

  it('shows "Approve all" only when more than one call still needs a decision', () => {
    const content = {
      text: null,
      requires_approval: true,
      tool_calls: [
        { id: 'a', name: 'edit_page', args: {}, requires_approval: true },
        { id: 'b', name: 'create_tag', args: {}, requires_approval: true },
        { id: 'c', name: 'search_pages', args: {}, resolved: true },
      ],
    }
    const wrapper = mount(AssistantMessageContent, {
      props: { role: 'assistant', content, messageId: 0 },
    })

    expect(wrapper.text()).toContain('Approve all')
  })
})
