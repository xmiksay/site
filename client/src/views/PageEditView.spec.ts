import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import PageEditView from './PageEditView.vue'
import { api, apiBlob } from '../api'
import type { PageDetail } from '../types'

// PageEditView drives the real pages/tags stores, so mock the HTTP layer
// underneath them rather than the stores themselves — same pattern as
// LoginView.spec.ts / pages.spec.ts.
vi.mock('../api', async (importActual) => {
  const actual = await importActual<typeof import('../api')>()
  return { ...actual, api: vi.fn(), apiVoid: vi.fn(), apiBlob: vi.fn() }
})

// Both the pages and tags stores wire `useListSync(topic, items)` at
// store-creation time, which registers a handler via `useWsStore().on(...)` —
// stub it out the same way pages.spec.ts does so mounting the view doesn't
// open a real WebSocket.
vi.mock('../stores/ws', () => ({
  useWsStore: () => ({ on: () => () => {} }),
}))

const pushMock = vi.fn()
vi.mock('vue-router', () => ({
  useRouter: () => ({ push: pushMock }),
}))

const apiMock = vi.mocked(api)
const apiBlobMock = vi.mocked(apiBlob)

function pageDetail(): PageDetail {
  return {
    id: 1,
    path: 'obsidian/rust',
    summary: 'A page',
    tag_ids: [],
    private: false,
    created_at: '2024-01-01',
    modified_at: '2024-01-01',
    markdown: '# Hello',
    revisions: [],
  }
}

function mountView() {
  return mount(PageEditView, {
    props: { id: '1' },
    global: {
      stubs: {
        PathPicker: true,
        MarkdownEditor: true,
        RouterLink: true,
      },
    },
  })
}

function flushPromises() {
  return new Promise((resolve) => setTimeout(resolve, 0))
}

describe('PageEditView export buttons', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('clicking Export PDF fetches the blob and triggers a download', async () => {
    apiMock.mockResolvedValueOnce([]) // tags.load
    apiMock.mockResolvedValueOnce(pageDetail()) // pages.read
    const blob = new Blob(['pdf-bytes'])
    apiBlobMock.mockResolvedValueOnce({ blob, filename: 'my-page.pdf' })

    const createObjectURLSpy = vi.fn(() => 'blob:mock-url')
    const revokeObjectURLSpy = vi.fn()
    vi.stubGlobal('URL', {
      ...URL,
      createObjectURL: createObjectURLSpy,
      revokeObjectURL: revokeObjectURLSpy,
    })
    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})

    const wrapper = mountView()
    await flushPromises()

    const exportButton = wrapper.findAll('button').find((b) => b.text() === 'Export PDF')
    expect(exportButton).toBeTruthy()
    await exportButton!.trigger('click')
    await flushPromises()

    expect(apiBlobMock).toHaveBeenCalledWith('/api/export/pages/1?format=pdf')
    expect(createObjectURLSpy).toHaveBeenCalledWith(blob)
    expect(clickSpy).toHaveBeenCalled()
    expect(revokeObjectURLSpy).toHaveBeenCalledWith('blob:mock-url')
    expect(wrapper.find('p.text-red-600').exists()).toBe(false)

    clickSpy.mockRestore()
    vi.unstubAllGlobals()
  })

  it('clicking Export slides requests the slides format', async () => {
    apiMock.mockResolvedValueOnce([]) // tags.load
    apiMock.mockResolvedValueOnce(pageDetail()) // pages.read
    apiBlobMock.mockResolvedValueOnce({ blob: new Blob(['html']), filename: 'my-page.html' })

    vi.stubGlobal('URL', {
      ...URL,
      createObjectURL: vi.fn(() => 'blob:mock-url'),
      revokeObjectURL: vi.fn(),
    })
    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})

    const wrapper = mountView()
    await flushPromises()

    const exportButton = wrapper.findAll('button').find((b) => b.text() === 'Export slides')
    await exportButton!.trigger('click')
    await flushPromises()

    expect(apiBlobMock).toHaveBeenCalledWith('/api/export/pages/1?format=slides')

    clickSpy.mockRestore()
    vi.unstubAllGlobals()
  })

  it('shows an error message when the export request fails', async () => {
    apiMock.mockResolvedValueOnce([]) // tags.load
    apiMock.mockResolvedValueOnce(pageDetail()) // pages.read
    apiBlobMock.mockRejectedValueOnce(new Error('export unavailable'))

    const wrapper = mountView()
    await flushPromises()

    const exportButton = wrapper.findAll('button').find((b) => b.text() === 'Export PDF')
    await exportButton!.trigger('click')
    await flushPromises()

    expect(wrapper.find('p.text-red-600').text()).toBe('export unavailable')
  })
})
