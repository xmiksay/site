import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import DesignPreview from './DesignPreview.vue'
import { useDesignStore } from '../stores/design'

describe('DesignPreview', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.useFakeTimers()
  })
  afterEach(() => vi.useRealTimers())

  it('turns draft preview on before showing the site', async () => {
    const setPreview = vi.spyOn(useDesignStore(), 'setPreview').mockResolvedValue()
    const wrapper = mount(DesignPreview)
    expect(setPreview).toHaveBeenCalledWith(true)
    await flushPromises()
    expect(wrapper.get('iframe').attributes('src')).toBe('/')
  })

  it('navigates the frame to the typed path, even back to where it started', async () => {
    vi.spyOn(useDesignStore(), 'setPreview').mockResolvedValue()
    const wrapper = mount(DesignPreview)
    await flushPromises()
    const assign = vi.fn()
    // The admin clicked a link inside the frame: it is no longer at `src`.
    const location = { pathname: '/blog/post', search: '', hash: '', assign, reload: vi.fn() }
    Object.defineProperty(wrapper.get('iframe').element, 'contentWindow', { value: { location } })
    await wrapper.get('iframe').trigger('load')
    expect((wrapper.get('input').element as HTMLInputElement).value).toBe('/blog/post')

    await wrapper.get('input').setValue('/')
    await wrapper.get('form').trigger('submit')
    expect(assign).toHaveBeenCalledWith('/')
    expect(location.reload).not.toHaveBeenCalled()
  })

  it('keeps a typed path on this origin, remounting when the frame is unreachable', async () => {
    vi.spyOn(useDesignStore(), 'setPreview').mockResolvedValue()
    const wrapper = mount(DesignPreview)
    await flushPromises()
    Object.defineProperty(wrapper.get('iframe').element, 'contentWindow', { value: null })
    await wrapper.get('input').setValue('/\t/evil.example/blog')
    await wrapper.get('form').trigger('submit')
    expect(wrapper.get('iframe').attributes('src')).toBe('/evil.example/blog')
  })

  it('reloads once per burst of draft changes', async () => {
    const store = useDesignStore()
    vi.spyOn(store, 'setPreview').mockResolvedValue()
    const wrapper = mount(DesignPreview, { attachTo: document.body })
    await flushPromises()
    const frame = wrapper.get('iframe').element as HTMLIFrameElement
    const reload = vi.fn()
    Object.defineProperty(frame, 'contentWindow', { value: { location: { reload, pathname: '/' } } })

    store.revision++
    await flushPromises()
    store.revision++
    await flushPromises()
    expect(reload).not.toHaveBeenCalled()
    vi.advanceTimersByTime(500)
    expect(reload).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })
})
