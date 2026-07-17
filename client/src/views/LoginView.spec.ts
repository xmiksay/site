import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { setActivePinia, createPinia } from 'pinia'
import LoginView from './LoginView.vue'
import { useAuthStore } from '../stores/auth'
import { api } from '../api'

// LoginView drives the real auth store (login/isLoggedIn), so mock the HTTP
// layer underneath it rather than the store itself — same pattern as the
// store specs, keeping the real ApiError export intact.
vi.mock('../api', async (importActual) => {
  const actual = await importActual<typeof import('../api')>()
  return { ...actual, api: vi.fn(), apiVoid: vi.fn() }
})

const apiMock = vi.mocked(api)

const pushMock = vi.fn()
vi.mock('vue-router', () => ({
  useRouter: () => ({ push: pushMock }),
}))

describe('LoginView', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
  })

  it('submits the entered credentials and redirects to /pages on success', async () => {
    apiMock.mockResolvedValueOnce({ user_id: 1, username: 'martin' })
    const wrapper = mount(LoginView)

    await wrapper.find('input[type="text"]').setValue('martin')
    await wrapper.find('input[type="password"]').setValue('secret')
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(apiMock).toHaveBeenCalledWith('/api/auth/login', {
      method: 'POST',
      body: JSON.stringify({ username: 'martin', password: 'secret' }),
    })
    expect(useAuthStore().isLoggedIn).toBe(true)
    expect(pushMock).toHaveBeenCalledWith('/pages')
    expect(wrapper.find('p.text-red-600').exists()).toBe(false)
  })

  it('shows the error message and does not redirect when login fails', async () => {
    apiMock.mockRejectedValueOnce(new Error('bad credentials'))
    const wrapper = mount(LoginView)

    await wrapper.find('input[type="text"]').setValue('martin')
    await wrapper.find('input[type="password"]').setValue('wrong')
    await wrapper.find('form').trigger('submit')
    await flushPromises()

    expect(pushMock).not.toHaveBeenCalled()
    expect(wrapper.find('p.text-red-600').text()).toBe('bad credentials')
    expect(useAuthStore().isLoggedIn).toBe(false)
  })

  it('disables the submit button while the request is in flight', async () => {
    let resolveLogin: (value: { user_id: number; username: string }) => void
    apiMock.mockReturnValueOnce(
      new Promise((resolve) => {
        resolveLogin = resolve
      }),
    )
    const wrapper = mount(LoginView)

    await wrapper.find('input[type="text"]').setValue('martin')
    await wrapper.find('input[type="password"]').setValue('secret')
    const submit = wrapper.find('form').trigger('submit')

    await wrapper.vm.$nextTick()
    const button = wrapper.find('button[type="submit"]')
    expect(button.attributes('disabled')).toBeDefined()
    expect(button.text()).toBe('…')

    resolveLogin!({ user_id: 1, username: 'martin' })
    await submit
    await flushPromises()

    expect(wrapper.find('button[type="submit"]').attributes('disabled')).toBeUndefined()
  })
})

function flushPromises() {
  return new Promise((resolve) => setTimeout(resolve, 0))
}
