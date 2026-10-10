import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import DesignStudioToolbar from './DesignStudioToolbar.vue'
import { useDesignStore } from '../stores/design'
import type { DesignState } from '../types'

const entry = { id: 'v1', at: '2026-10-10T12:00:00Z', by: 'me', files: 4 }

function state(changes: DesignState['changes'] = [{ path: 'templates/base.html', kind: 'modified' }]): DesignState {
  return { storage: 'db', local_dir: false, last_reload: null, initialized: true, files: [], changes }
}

function button(wrapper: ReturnType<typeof mount>, label: string) {
  return wrapper.findAll('button').find((b) => b.text() === label)!
}

describe('DesignStudioToolbar', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    useDesignStore().state = state()
  })
  afterEach(() => vi.unstubAllGlobals())

  it('shows the change count and publishes', async () => {
    const store = useDesignStore()
    const publish = vi.spyOn(store, 'publish').mockResolvedValue({ kind: 'published', entry })
    const wrapper = mount(DesignStudioToolbar, { props: { dirty: false } })
    expect(wrapper.get('[data-test="change-count"]').text()).toBe('1 unpublished change(s)')

    await button(wrapper, 'Publikovat').trigger('click')
    await flushPromises()
    expect(publish).toHaveBeenCalledWith()
    expect(wrapper.text()).toContain('(4 files) is live')
  })

  it('lists 422 render-check errors with their template location', async () => {
    vi.spyOn(useDesignStore(), 'publish').mockResolvedValue({
      kind: 'invalid',
      errors: ['templates/base.html:3: undefined value (rendering 404)', 'templates/page.html: syntax error'],
    })
    const wrapper = mount(DesignStudioToolbar, { props: { dirty: false } })
    await button(wrapper, 'Publikovat').trigger('click')
    await flushPromises()

    const items = wrapper.findAll('li')
    expect(items).toHaveLength(2)
    expect(items[0].get('span').text()).toBe('templates/base.html:3')
    expect(items[0].text()).toContain('undefined value (rendering 404)')
    expect(wrapper.text()).toContain('failed the render check')
  })

  it('a 409 conflict lists the paths and forces only after the confirm', async () => {
    const confirm = vi.fn().mockReturnValue(true)
    vi.stubGlobal('confirm', confirm)
    const publish = vi
      .spyOn(useDesignStore(), 'publish')
      .mockResolvedValueOnce({ kind: 'conflict', paths: ['assets/css/style.css'], message: 'x' })
      .mockResolvedValueOnce({ kind: 'published', entry })
    const wrapper = mount(DesignStudioToolbar, { props: { dirty: false } })
    await button(wrapper, 'Publikovat').trigger('click')
    await flushPromises()

    expect(confirm.mock.calls[0][0]).toContain('• assets/css/style.css')
    expect(publish).toHaveBeenNthCalledWith(2, true)
    expect(wrapper.text()).toContain('is live')
  })

  it('a declined conflict confirm does not force', async () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(false))
    const publish = vi
      .spyOn(useDesignStore(), 'publish')
      .mockResolvedValue({ kind: 'conflict', paths: ['a'], message: 'x' })
    const wrapper = mount(DesignStudioToolbar, { props: { dirty: false } })
    await button(wrapper, 'Publikovat').trigger('click')
    await flushPromises()
    expect(publish).toHaveBeenCalledTimes(1)
  })

  it('nothing to publish is an info, not an error', async () => {
    vi.spyOn(useDesignStore(), 'publish').mockResolvedValue({ kind: 'nothing', message: 'nothing to publish' })
    const wrapper = mount(DesignStudioToolbar, { props: { dirty: false } })
    await button(wrapper, 'Publikovat').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('Nothing to publish')
    expect(wrapper.find('.text-danger-strong').exists()).toBe(false)
  })

  it('unsaved editor changes ask before publishing', async () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(false))
    const publish = vi.spyOn(useDesignStore(), 'publish')
    const wrapper = mount(DesignStudioToolbar, { props: { dirty: true } })
    await button(wrapper, 'Publikovat').trigger('click')
    expect(publish).not.toHaveBeenCalled()
  })

  it('Zahodit draft confirms, discards and resets the editor', async () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(true))
    const discard = vi.spyOn(useDesignStore(), 'discard').mockResolvedValue()
    const wrapper = mount(DesignStudioToolbar, { props: { dirty: false } })
    await button(wrapper, 'Zahodit draft').trigger('click')
    await flushPromises()
    expect(discard).toHaveBeenCalled()
    expect(wrapper.emitted('reset')).toHaveLength(1)
  })

  it('discard and restore warn that the open file loses its unsaved edits', async () => {
    const confirm = vi.fn().mockReturnValue(false)
    vi.stubGlobal('confirm', confirm)
    const store = useDesignStore()
    vi.spyOn(store, 'loadHistory').mockImplementation(async () => {
      store.history = [entry]
    })
    const restore = vi.spyOn(store, 'restore')
    const wrapper = mount(DesignStudioToolbar, { props: { dirty: true } })

    await button(wrapper, 'Zahodit draft').trigger('click')
    expect(confirm.mock.calls[0][0]).toContain('unsaved edits are lost')

    await button(wrapper, 'Historie').trigger('click')
    await flushPromises()
    await button(wrapper, 'Restore to draft').trigger('click')
    expect(confirm.mock.calls[1][0]).toContain('unsaved edits are lost')
    expect(restore).not.toHaveBeenCalled()
  })

  it('Zahodit draft is disabled without changes', () => {
    useDesignStore().state = state([])
    const wrapper = mount(DesignStudioToolbar, { props: { dirty: false } })
    expect(button(wrapper, 'Zahodit draft').attributes('disabled')).toBeDefined()
  })

  it('Historie lists versions and restores one into the draft after the confirm', async () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(true))
    const store = useDesignStore()
    vi.spyOn(store, 'loadHistory').mockImplementation(async () => {
      store.history = [entry]
    })
    const restore = vi.spyOn(store, 'restore').mockResolvedValue()
    const wrapper = mount(DesignStudioToolbar, { props: { dirty: false } })
    await button(wrapper, 'Historie').trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('by me · 4 files')

    await button(wrapper, 'Restore to draft').trigger('click')
    await flushPromises()
    expect(restore).toHaveBeenCalledWith('v1')
    expect(wrapper.emitted('reset')).toHaveLength(1)
    expect(wrapper.text()).toContain('restored into the draft')
  })
})
