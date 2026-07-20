import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { reactive } from 'vue'
import LiveToolCallList from './LiveToolCallList.vue'
import { useAssistantStore } from '../stores/assistant'
import type { LiveToolCall } from '../types'

// This component drives the live/streaming turn's tool-call approval flow —
// the bug it fixes was the buttons staying stuck forever with no feedback if
// the click's POST failed, or if the trailing `tool_output` WS event that
// used to be the only thing clearing them never arrived. Mock the store
// directly (rather than the HTTP layer underneath it, like the store specs
// do) since what matters here is the component's *reaction* to
// `approveToolCalls` resolving/rejecting and to `resolveLiveToolCall` being
// called — not re-testing the store's own HTTP plumbing.
vi.mock('../stores/assistant', () => ({
  useAssistantStore: vi.fn(),
}))

const useAssistantStoreMock = vi.mocked(useAssistantStore)

function makeCall(id: string): LiveToolCall {
  return { id, name: 'page_delete', argsText: '{"id":1}', args: { id: 1 }, status: 'requires_approval' }
}

describe('LiveToolCallList', () => {
  let approveToolCalls: ReturnType<typeof vi.fn>
  let resolveLiveToolCall: ReturnType<typeof vi.fn>
  let toolCalls: LiveToolCall[]

  beforeEach(() => {
    // `reactive` (not a plain array) so the mocked `resolveLiveToolCall`
    // mutating a call's `.status` in place is actually visible to the
    // mounted component — mirrors how the real store's `live.toolCalls`
    // entries are reactive Pinia state shared by reference with whichever
    // view renders them.
    toolCalls = reactive([makeCall('c1')]) as unknown as LiveToolCall[]
    approveToolCalls = vi.fn().mockResolvedValue(undefined)
    resolveLiveToolCall = vi.fn((callId: string) => {
      const call = toolCalls.find((c) => c.id === callId)
      if (call) call.status = 'done'
    })
    useAssistantStoreMock.mockReturnValue({ approveToolCalls, resolveLiveToolCall } as any)
  })

  it('clicking Allow resolves the call locally without waiting for a WS event', async () => {
    const wrapper = mount(LiveToolCallList, { props: { toolCalls, sessionId: 1 } })

    await wrapper.find('button').trigger('click')
    await flushPromises()

    expect(approveToolCalls).toHaveBeenCalledWith(1, 0, [
      { tool_call_id: 'c1', approve: true, remember: false },
    ])
    expect(resolveLiveToolCall).toHaveBeenCalledWith('c1', undefined)
    // Status flipped to 'done' synchronously on the approve response, not on
    // a later tool_output — the approval buttons are gone immediately.
    expect(wrapper.find('button').exists()).toBe(false)
    expect(wrapper.text()).toContain('✓')
  })

  it('a rejected POST leaves the button enabled and shows an error', async () => {
    approveToolCalls.mockRejectedValueOnce(new Error('network down'))
    const wrapper = mount(LiveToolCallList, { props: { toolCalls, sessionId: 1 } })

    const approveButton = wrapper.findAll('button')[0]
    await approveButton.trigger('click')
    await flushPromises()

    expect(resolveLiveToolCall).not.toHaveBeenCalled()
    expect(wrapper.text()).toContain('network down')
    // Buttons are still rendered (status never left requires_approval) and
    // re-enabled once the in-flight request settles.
    expect(wrapper.findAll('button')[0].attributes('disabled')).toBeUndefined()
  })

  it('deciding one of two pending calls does not disable the other', async () => {
    toolCalls.push(makeCall('c2'))
    let resolveApprove!: () => void
    approveToolCalls.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          resolveApprove = resolve
        }),
    )
    const wrapper = mount(LiveToolCallList, { props: { toolCalls, sessionId: 1 } })

    const before = wrapper.findAll('button')
    expect(before).toHaveLength(8) // 4 buttons per pending call

    await before[0].trigger('click') // Approve on c1
    await wrapper.vm.$nextTick()

    const mid = wrapper.findAll('button')
    expect(mid[0].attributes('disabled')).toBeDefined() // c1's Approve: in flight
    expect(mid[4].attributes('disabled')).toBeUndefined() // c2's Approve: untouched

    resolveApprove()
    await flushPromises()

    expect(resolveLiveToolCall).toHaveBeenCalledWith('c1', undefined)
    expect(resolveLiveToolCall).not.toHaveBeenCalledWith('c2', undefined)
  })

  it('threads agentSessionId through to resolveLiveToolCall for sub-agent calls', async () => {
    const wrapper = mount(LiveToolCallList, {
      props: { toolCalls, sessionId: 1, agentSessionId: 'child-1' },
    })

    await wrapper.find('button').trigger('click')
    await flushPromises()

    expect(resolveLiveToolCall).toHaveBeenCalledWith('c1', 'child-1')
  })
})

function flushPromises() {
  return new Promise((resolve) => setTimeout(resolve, 0))
}
