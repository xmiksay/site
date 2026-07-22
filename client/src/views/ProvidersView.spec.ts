import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import ProvidersView from './ProvidersView.vue'
import { api } from '../api'
import type { LlmProvider, ProviderThrottleStatus } from '../types'

// ProvidersView drives the real assistant store, so mock the HTTP layer
// underneath it rather than the store itself — same pattern as
// LoginView.spec.ts / PageEditView.spec.ts.
vi.mock('../api', async (importActual) => {
  const actual = await importActual<typeof import('../api')>()
  return { ...actual, api: vi.fn(), apiVoid: vi.fn() }
})

const apiMock = vi.mocked(api)

function provider(): LlmProvider {
  return {
    id: 1,
    label: 'Anthropic',
    kind: 'anthropic',
    base_url: null,
    has_api_key: true,
    concurrency: null,
    rpm: null,
    created_at: '2024-01-01',
  }
}

function status(overrides: Partial<ProviderThrottleStatus> = {}): ProviderThrottleStatus {
  return {
    provider_id: 1,
    endpoint: 'https://api.anthropic.com',
    in_flight: 0,
    cap: 3,
    backoff_remaining_ms: null,
    penalized: false,
    ...overrides,
  }
}

function mountView() {
  return mount(ProvidersView, {
    global: { stubs: { RouterLink: true } },
  })
}

function flushPromises() {
  return new Promise((resolve) => setTimeout(resolve, 0))
}

describe('ProvidersView throttle status', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('fetches both providers and throttle status on mount', async () => {
    apiMock.mockResolvedValueOnce([provider()])
    apiMock.mockResolvedValueOnce([status()])

    mountView()
    await flushPromises()

    expect(apiMock).toHaveBeenCalledWith('/api/assistant/providers')
    expect(apiMock).toHaveBeenCalledWith('/api/assistant/providers/status')
  })

  it('renders "idle" when the provider is not throttled', async () => {
    apiMock.mockResolvedValueOnce([provider()])
    apiMock.mockResolvedValueOnce([status()])

    const wrapper = mountView()
    await flushPromises()
    await wrapper.vm.$nextTick()

    expect(wrapper.text()).toContain('idle')
  })

  it('renders a backing-off countdown when a cool-down is active', async () => {
    apiMock.mockResolvedValueOnce([provider()])
    apiMock.mockResolvedValueOnce([status({ backoff_remaining_ms: 12000 })])

    const wrapper = mountView()
    await flushPromises()
    await wrapper.vm.$nextTick()

    expect(wrapper.text()).toContain('backing off 12s')
  })

  it('renders "slowed" when penalized with no active cool-down', async () => {
    apiMock.mockResolvedValueOnce([provider()])
    apiMock.mockResolvedValueOnce([status({ penalized: true, backoff_remaining_ms: null })])

    const wrapper = mountView()
    await flushPromises()
    await wrapper.vm.$nextTick()

    expect(wrapper.text()).toContain('slowed')
  })

  it('renders "at capacity" when saturated at the in-flight cap', async () => {
    apiMock.mockResolvedValueOnce([provider()])
    apiMock.mockResolvedValueOnce([
      status({ in_flight: 3, cap: 3, penalized: false, backoff_remaining_ms: null }),
    ])

    const wrapper = mountView()
    await flushPromises()
    await wrapper.vm.$nextTick()

    expect(wrapper.text()).toContain('at capacity (3/3)')
  })

  it('renders "idle" (not "undefined") when no status entry exists for the provider', async () => {
    apiMock.mockResolvedValueOnce([provider()])
    apiMock.mockResolvedValueOnce([])

    const wrapper = mountView()
    await flushPromises()
    await wrapper.vm.$nextTick()

    expect(wrapper.text()).toContain('idle')
    expect(wrapper.text()).not.toContain('undefined')
  })

  it('polls the status endpoint every 5s and stops after unmount', async () => {
    vi.useFakeTimers()
    apiMock.mockResolvedValue([provider()])

    const wrapper = mountView()
    // Flush the async onMounted body (initial loadProviders + loadThrottleStatuses)
    // without relying on real timers, since fake timers are active.
    await vi.advanceTimersByTimeAsync(0)

    const statusCallsAfterMount = apiMock.mock.calls.filter(
      (c) => c[0] === '/api/assistant/providers/status',
    ).length
    expect(statusCallsAfterMount).toBe(1)

    await vi.advanceTimersByTimeAsync(5000)
    const statusCallsAfterOnePoll = apiMock.mock.calls.filter(
      (c) => c[0] === '/api/assistant/providers/status',
    ).length
    expect(statusCallsAfterOnePoll).toBe(2)

    wrapper.unmount()
    await vi.advanceTimersByTimeAsync(20000)
    const statusCallsAfterUnmount = apiMock.mock.calls.filter(
      (c) => c[0] === '/api/assistant/providers/status',
    ).length
    expect(statusCallsAfterUnmount).toBe(2)
  })
})
