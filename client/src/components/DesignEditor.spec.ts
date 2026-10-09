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

    const wrapper = mount(DesignEditor, { props: { file: overridden, editable: true } })
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

    const wrapper = mount(DesignEditor, { props: { file, editable: true } })
    await flushPromises()
    expect(wrapper.get('textarea').attributes('readonly')).toBeDefined()

    const override = wrapper.findAll('button').find((b) => b.text() === 'Override')!
    await override.trigger('click')
    await flushPromises()

    expect(fetchText).toHaveBeenLastCalledWith('templates/base.html', true)
    expect(wrapper.get('textarea').attributes('readonly')).toBeUndefined()
    expect(wrapper.emitted('dirty')!.at(-1)).toEqual([true])
  })
})
