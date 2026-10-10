import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useColorScheme } from './colorScheme'

function fakeMedia(dark: boolean) {
  const listeners: Array<() => void> = []
  const media = {
    matches: dark,
    addEventListener: (_: string, cb: () => void) => listeners.push(cb),
    flip(next: boolean) {
      media.matches = next
      listeners.forEach((cb) => cb())
    },
  }
  vi.stubGlobal('matchMedia', () => media)
  return media
}

function fakeStorage(initial: Record<string, string> = {}) {
  const data = { ...initial }
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => data[k] ?? null,
    setItem: (k: string, v: string) => {
      data[k] = v
    },
  })
  return data
}

const applied = () => document.documentElement.getAttribute('data-scheme')

describe('useColorScheme', () => {
  beforeEach(() => document.documentElement.removeAttribute('data-scheme'))
  afterEach(() => vi.unstubAllGlobals())

  it('follows the OS scheme when nothing is saved, and keeps following it', () => {
    const media = fakeMedia(true)
    const storage = fakeStorage()
    const { scheme } = useColorScheme()
    expect(scheme.value).toBe('dark')
    expect(applied()).toBe('dark')

    media.flip(false)
    expect(scheme.value).toBe('light')
    expect(applied()).toBe('light')
    expect(storage.dataScheme).toBeUndefined()
  })

  it('prefers the saved choice over the OS', () => {
    const media = fakeMedia(true)
    fakeStorage({ dataScheme: 'light' })
    const { scheme } = useColorScheme()
    expect(scheme.value).toBe('light')

    media.flip(false)
    media.flip(true)
    expect(scheme.value).toBe('light')
  })

  it('remembers a choice and stops following the OS', () => {
    const media = fakeMedia(false)
    const storage = fakeStorage()
    const { scheme, choose } = useColorScheme()
    choose('dark')
    expect(storage.dataScheme).toBe('dark')
    expect(applied()).toBe('dark')

    media.flip(false)
    expect(scheme.value).toBe('dark')
  })

  it('survives storage that throws', () => {
    fakeMedia(false)
    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('blocked')
      },
      setItem: () => {
        throw new Error('blocked')
      },
    })
    const { scheme, choose } = useColorScheme()
    expect(scheme.value).toBe('light')
    choose('dark')
    expect(scheme.value).toBe('dark')
  })
})
