import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import AssistantSessionTree from './AssistantSessionTree.vue'
import type { AssistantSession } from '../types'

function session(id: number, overrides: Partial<AssistantSession> = {}): AssistantSession {
  return {
    id,
    title: `chat ${id}`,
    provider: 'anthropic',
    model: 'sonnet',
    model_id: 1,
    enabled_mcp_server_ids: [],
    temperature: null,
    reasoning_effort: null,
    max_output_tokens: null,
    thinking_budget_tokens: null,
    agent_profile: 'build',
    parent_session_id: null,
    root_engine_session_id: 'u1:root',
    created_at: '2026-08-01T10:00:00Z',
    updated_at: '2026-08-01T10:00:00Z',
    ...overrides,
  }
}

// The title line: a child prefixes its profile icon, so collapse the markup's
// own whitespace before comparing.
const rowTitles = (wrapper: ReturnType<typeof mount>) =>
  wrapper
    .findAll('[role="treeitem"]')
    .map((r) => r.find('.text-sm').text().replace(/\s+/g, ' ').trim())

const sessions = [
  session(1, { title: 'root', updated_at: '2026-08-01T12:00:00Z' }),
  session(2, { title: 'kid', parent_session_id: 1, agent_profile: 'researcher' }),
  session(3, { title: 'grandkid', parent_session_id: 2, agent_profile: 'page-writer' }),
  session(4, { title: 'other root', updated_at: '2026-08-01T09:00:00Z' }),
]

function mountTree(currentId: number | null = null, list = sessions) {
  return mount(AssistantSessionTree, { props: { sessions: list, currentId } })
}

describe('AssistantSessionTree', () => {
  it('shows roots only until a branch is expanded — a spawn-heavy chat must not bury them', () => {
    const wrapper = mountTree()

    expect(rowTitles(wrapper)).toEqual(['root', 'other root'])
    // The collapsed root still says how much is hidden under it.
    expect(wrapper.text()).toContain('1 sub-agent')
  })

  it('expands a branch one level at a time, indenting and icon-tagging the children', async () => {
    const wrapper = mountTree()

    await wrapper.get('[aria-label="Expand root"]').trigger('click')
    expect(rowTitles(wrapper)).toEqual(['root', '🔎 kid', 'other root'])

    await wrapper.get('[aria-label="Expand kid"]').trigger('click')
    expect(rowTitles(wrapper)).toEqual(['root', '🔎 kid', '✎ grandkid', 'other root'])

    // Depth reads off both the guide rules and aria-level.
    const rows = wrapper.findAll('[role="treeitem"]')
    expect(rows.map((r) => r.attributes('aria-level'))).toEqual(['1', '2', '3', '1'])
    expect(rows[2].findAll('.border-r')).toHaveLength(2)

    await wrapper.get('[aria-label="Collapse root"]').trigger('click')
    expect(rowTitles(wrapper)).toEqual(['root', 'other root'])
  })

  it('auto-expands the ancestors of the selected session so a child is visible', () => {
    const wrapper = mountTree(3)

    expect(rowTitles(wrapper)).toEqual(['root', '🔎 kid', '✎ grandkid', 'other root'])
    const selected = wrapper.findAll('[role="treeitem"]')[2]
    expect(selected.attributes('aria-selected')).toBe('true')
    expect(selected.classes()).toContain('bg-surface-raised')
  })

  it('emits select with the clicked row id', async () => {
    const wrapper = mountTree(3)

    await wrapper.findAll('[role="treeitem"]')[1].findAll('button')[1].trigger('click')
    expect(wrapper.emitted('select')).toEqual([[2]])
  })

  it('offers delete on roots only — a child delete is a 409 server-side', () => {
    const wrapper = mountTree(3)

    expect(wrapper.findAll('[aria-label^="Delete"]').map((b) => b.attributes('aria-label'))).toEqual(
      ['Delete root', 'Delete other root'],
    )
  })

  it('renders an orphaned child at root level, still without a delete button', () => {
    const wrapper = mountTree(null, [
      session(1, { title: 'root' }),
      session(9, { title: 'lost kid', parent_session_id: 404, agent_profile: 'researcher' }),
    ])

    expect(rowTitles(wrapper)).toContain('🔎 lost kid')
    expect(wrapper.findAll('[aria-label^="Delete"]').map((b) => b.attributes('aria-label'))).toEqual(
      ['Delete root'],
    )
  })

  it('emits delete for a root', async () => {
    const wrapper = mountTree()

    await wrapper.get('[aria-label="Delete other root"]').trigger('click')
    expect(wrapper.emitted('delete')).toEqual([[4]])
  })

  it('renders an empty state with no sessions', () => {
    expect(mountTree(null, []).text()).toBe('No chats yet.')
  })
})
