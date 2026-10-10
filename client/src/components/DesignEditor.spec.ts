import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import DesignEditor from './DesignEditor.vue'
import { useDesignStore } from '../stores/design'
import { ApiError } from '../api'
import type { DesignFile } from '../types'

const overridden: DesignFile = { path: 'templates/base.html', baked: true, overridden: true, size: 5 }

describe('DesignEditor', () => {
  beforeEach(() => setActivePinia(createPinia()))

  it('keeps the edited content and shows the error when save fails with 422', async () => {
    const store = useDesignStore()
    vi.spyOn(store, 'fetchText').mockResolvedValue('<old>')
    vi.spyOn(store, 'save').mockRejectedValue(new ApiError(422, 'base.html: unexpected end'))

    const wrapper = mount(DesignEditor, { props: { file: overridden } })
    await flushPromises()
    const textarea = wrapper.get('textarea')
    await textarea.setValue('{% broken')
    await wrapper.get('button:not([disabled])').trigger('click')
    await flushPromises()

    expect(store.save).toHaveBeenCalledWith('templates/base.html', '{% broken')
    expect((textarea.element as HTMLTextAreaElement).value).toBe('{% broken')
    expect(wrapper.text()).toContain('base.html: unexpected end')
    expect(wrapper.text()).toContain('unsaved changes')
  })

  it('baked-only file is read-only until Override seeds it from the baked source', async () => {
    const store = useDesignStore()
    const fetchText = vi.spyOn(store, 'fetchText').mockResolvedValue('<baked>')
    const file = { ...overridden, overridden: false }

    const wrapper = mount(DesignEditor, { props: { file } })
    await flushPromises()
    expect(wrapper.get('textarea').attributes('readonly')).toBeDefined()

    const override = wrapper.findAll('button').find((b) => b.text() === 'Override')!
    await override.trigger('click')
    await flushPromises()

    expect(fetchText).toHaveBeenLastCalledWith('templates/base.html', 'baked')
    expect(wrapper.get('textarea').attributes('readonly')).toBeUndefined()
    expect(wrapper.emitted('dirty')!.at(-1)).toEqual([true])
  })

  it('picks up a draft change made elsewhere unless the file has unsaved edits', async () => {
    const store = useDesignStore()
    const fetchText = vi.spyOn(store, 'fetchText').mockResolvedValue('<old>')
    const wrapper = mount(DesignEditor, { props: { file: overridden } })
    await flushPromises()

    fetchText.mockResolvedValue('<from the AI>')
    store.revision++
    await flushPromises()
    const textarea = wrapper.get('textarea')
    expect((textarea.element as HTMLTextAreaElement).value).toBe('<from the AI>')

    await textarea.setValue('<mine>')
    fetchText.mockResolvedValue('<again>')
    store.revision++
    await flushPromises()
    expect((textarea.element as HTMLTextAreaElement).value).toBe('<mine>')
  })

  it('shows the published version read-only, keeping the draft edit', async () => {
    const store = useDesignStore()
    vi.spyOn(store, 'fetchText').mockResolvedValue('<draft>')
    const fetchContent = vi.spyOn(store, 'fetchContent').mockResolvedValue(new Blob(['<live>']))
    const wrapper = mount(DesignEditor, { props: { file: overridden } })
    await flushPromises()
    await wrapper.get('textarea').setValue('<edited>')

    await wrapper.get('select').setValue('published')
    await flushPromises()
    expect(fetchContent).toHaveBeenCalledWith('templates/base.html', 'published')
    const view = wrapper.get('[data-test="source-view"]')
    expect((view.element as HTMLTextAreaElement).value).toBe('<live>')
    expect(view.attributes('readonly')).toBeDefined()

    await wrapper.get('select').setValue('draft')
    expect((wrapper.get('textarea').element as HTMLTextAreaElement).value).toBe('<edited>')
  })

  it('says when the file is not in the published design', async () => {
    const store = useDesignStore()
    vi.spyOn(store, 'fetchText').mockResolvedValue('<draft>')
    vi.spyOn(store, 'fetchContent').mockRejectedValue(new ApiError(404, 'not found'))
    const wrapper = mount(DesignEditor, { props: { file: overridden } })
    await flushPromises()
    await wrapper.get('select').setValue('published')
    await flushPromises()
    expect(wrapper.text()).toContain('Not in the published design.')
  })
})
