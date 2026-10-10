import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import AssistantMessageContent from './AssistantMessageContent.vue'
import { useAssistantStore } from '../stores/assistant'

// Regression coverage for the "historic tool prompt looks unresolved, but
// it's solved in the background" bug: this persisted/REST message view used
// to gate every button on the message-level `requires_approval` flag plus
// "no decision recorded yet", which can't distinguish a genuinely-pending
// call from an auto-allowed sibling or an orphaned decision (see
// `useAssistantContent.spec.ts` and `src/ai/projection/turn.rs`'s
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
        { id: 'gated', name: 'page_edit', args: { path: 'x' }, requires_approval: true },
        { id: 'auto', name: 'page_search', args: { q: 'x' } },
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
      tool_calls: [{ id: 'a', name: 'page_edit', args: {}, requires_approval: true, resolved: true }],
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
      tool_calls: [{ id: 'a', name: 'page_edit', args: {}, requires_approval: true }],
    }
    const wrapper = mount(AssistantMessageContent, {
      props: { role: 'assistant', content, messageId: 0 },
    })

    await wrapper.find('button').trigger('click')
    expect(approveToolCalls).toHaveBeenCalledWith(1, 0, [{ tool_call_id: 'a', approve: true, remember: false }])
  })

  it('renders a closed "Thinking" disclosure when the message carries reasoning', () => {
    const wrapper = mount(AssistantMessageContent, {
      props: {
        role: 'assistant',
        content: { text: '4', reasoning: 'Let me add them.', tool_calls: [] },
        messageId: 0,
      },
    })

    const details = wrapper.find('details')
    expect(details.exists()).toBe(true)
    // Closed by default — the answer must stay the first thing on screen.
    expect(details.attributes('open')).toBeUndefined()
    expect(details.find('summary').text()).toBe('Thinking')
    expect(details.text()).toContain('Let me add them.')
  })

  it('omits the "Thinking" disclosure when the message has no reasoning', () => {
    const wrapper = mount(AssistantMessageContent, {
      props: { role: 'assistant', content: { text: '4', tool_calls: [] }, messageId: 0 },
    })

    expect(wrapper.find('details').exists()).toBe(false)
    expect(wrapper.text()).not.toContain('Thinking')
  })

  it('shows "Approve all" only when more than one call still needs a decision', () => {
    const content = {
      text: null,
      requires_approval: true,
      tool_calls: [
        { id: 'a', name: 'page_edit', args: {}, requires_approval: true },
        { id: 'b', name: 'tag_create', args: {}, requires_approval: true },
        { id: 'c', name: 'page_search', args: {}, resolved: true },
      ],
    }
    const wrapper = mount(AssistantMessageContent, {
      props: { role: 'assistant', content, messageId: 0 },
    })

    expect(wrapper.text()).toContain('Approve all')
  })

  // #100: a sub-agent contributes a reference card, never a nested
  // transcript. The old recursive `<details>` rendered nothing at all for a
  // grandchild; the card has to read on its own, without a click.
  function subAgentContent(card: Record<string, any>) {
    return {
      text: null,
      tool_calls: [{ id: 'spawn-1', name: 'agent_spawn', args: {}, resolved: true }],
      sub_agents: [card],
    }
  }

  const card = {
    agent_id: 'a-uuid',
    profile: 'researcher',
    task: 'look into X',
    message_count: 4,
    preview: 'X is interesting.',
    child_db_session_id: 7,
  }

  it('renders a sub-agent as a summary card, not a nested transcript', () => {
    const wrapper = mount(AssistantMessageContent, {
      props: { role: 'assistant', content: subAgentContent(card), messageId: 0 },
    })

    expect(wrapper.text()).toContain('researcher')
    expect(wrapper.text()).toContain('look into X')
    expect(wrapper.text()).toContain('4 messages')
    expect(wrapper.text()).toContain('X is interesting.')
  })

  // #103: the card is the way into the child's own session — the view owns
  // selection, so the card only emits.
  it('emits selectSession with the card’s child_db_session_id when clicked', async () => {
    const wrapper = mount(AssistantMessageContent, {
      props: { role: 'assistant', content: subAgentContent(card), messageId: 0 },
    })

    const button = wrapper.get('button')
    expect(button.text()).toContain('researcher')
    await button.trigger('click')
    expect(wrapper.emitted('selectSession')).toEqual([[7]])
  })

  it('renders a card without child_db_session_id, but inert', async () => {
    const orphan = { ...card, child_db_session_id: undefined }
    const wrapper = mount(AssistantMessageContent, {
      props: { role: 'assistant', content: subAgentContent(orphan), messageId: 0 },
    })

    expect(wrapper.text()).toContain('look into X')
    // No click target at all — a card that can't be opened must not look like
    // one that can.
    expect(wrapper.find('button').exists()).toBe(false)
    await wrapper.get('div.border-line-2').trigger('click')
    expect(wrapper.emitted('selectSession')).toBeUndefined()
  })
})
