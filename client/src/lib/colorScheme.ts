import { ref, type Ref } from 'vue'
import type { DataScheme } from '../types'

const KEY = 'dataScheme'

// Storage can throw (private mode, blocked site data); the scheme then just
// isn't remembered.
function readSaved(): DataScheme | null {
  try {
    const value = localStorage.getItem(KEY)
    return value === 'light' || value === 'dark' ? value : null
  } catch {
    return null
  }
}

/**
 * The admin colour scheme: the user's saved choice, otherwise the OS
 * preference — followed live until the user picks a scheme themselves.
 */
export function useColorScheme(): { scheme: Ref<DataScheme>; choose: (scheme: DataScheme) => void } {
  const media = window.matchMedia?.('(prefers-color-scheme: dark)')
  const system = (): DataScheme => (media?.matches ? 'dark' : 'light')
  const saved = readSaved()
  const scheme = ref<DataScheme>(saved ?? system())
  let followSystem = saved === null

  const apply = () => document.documentElement.setAttribute('data-scheme', scheme.value)
  apply()

  media?.addEventListener?.('change', () => {
    if (!followSystem) return
    scheme.value = system()
    apply()
  })

  function choose(next: DataScheme) {
    followSystem = false
    scheme.value = next
    apply()
    try {
      localStorage.setItem(KEY, next)
    } catch {
      // Not remembered; the choice still holds for this page load.
    }
  }

  return { scheme, choose }
}
