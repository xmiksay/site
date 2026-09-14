<script setup lang="ts">
import { ref, watch, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { useAuthStore } from './stores/auth'
import { useWsStore } from './stores/ws'
import type { DataScheme } from './types'

const auth = useAuthStore()
const ws = useWsStore()
const router = useRouter()

const mobileOpen = ref(false)
const dataScheme = ref<DataScheme>('light')

watch(() => router.currentRoute.value.fullPath, () => {
  mobileOpen.value = false
})

watch(
  () => auth.isLoggedIn,
  (loggedIn) => {
    if (loggedIn) ws.connect()
    else ws.disconnect()
  },
  { immediate: true },
)

watch(dataScheme, (scheme) => {
  applyDataScheme(scheme)
  localStorage.setItem('dataScheme', scheme)
})

onMounted(() => {
  const savedScheme = localStorage.getItem('dataScheme')
  const scheme = savedScheme === 'light' ? 'light' : 'dark'

  applyDataScheme(scheme)
  dataScheme.value = scheme
})

async function handleLogout() {
  await auth.logout()
  router.push('/login')
}

function applyDataScheme(scheme: DataScheme) {
  document.documentElement.setAttribute('data-scheme', scheme)
}
</script>

<template>
  <div class="min-h-screen bg-surface-alt text-fg-1">
    <div v-if="auth.isLoggedIn" class="md:flex md:min-h-screen">
      <header
        class="md:hidden flex items-center justify-between bg-surface-dark text-fg-on-dark px-4 py-3"
      >
        <a href="/" class="font-semibold">Site</a>
        <button
          type="button"
          class="p-2 rounded hover:bg-surface-dark-hover focus:outline-none focus:ring-2 focus:ring-focus"
          aria-label="Toggle navigation"
          :aria-expanded="mobileOpen"
          @click="mobileOpen = !mobileOpen"
        >
          <svg
            v-if="!mobileOpen"
            xmlns="http://www.w3.org/2000/svg"
            class="h-6 w-6"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
          >
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 6h16M4 12h16M4 18h16" />
          </svg>
          <svg
            v-else
            xmlns="http://www.w3.org/2000/svg"
            class="h-6 w-6"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
          >
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
          </svg>
        </button>
      </header>

      <div
        v-if="mobileOpen"
        class="md:hidden fixed inset-0 z-30 bg-overlay"
        @click="mobileOpen = false"
      ></div>

      <aside
        class="bg-surface-dark text-fg-on-dark flex flex-col z-40
               fixed inset-y-0 left-0 w-64 transform transition-transform duration-200 ease-out
               md:static md:w-56 md:translate-x-0"
        :class="mobileOpen ? 'translate-x-0' : '-translate-x-full md:translate-x-0'"
      >
        <div class="px-4 py-4 border-b border-line-on-dark font-semibold flex items-center justify-between">
          <a href="/">Site</a>
          <button
            type="button"
            class="md:hidden p-1 rounded hover:bg-surface-dark-hover"
            aria-label="Close navigation"
            @click="mobileOpen = false"
          >
            <svg xmlns="http://www.w3.org/2000/svg" class="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
        <nav class="flex-1 px-2 py-3 space-y-1 text-sm overflow-y-auto">
          <router-link
            v-for="item in nav"
            :key="item.to"
            :to="item.to"
            class="block px-3 py-2 rounded hover:bg-surface-dark-hover"
            active-class="bg-surface-dark-hover font-medium"
          >
            {{ item.label }}
          </router-link>
        </nav>
        <div class="px-4 py-3 border-t border-line-on-dark text-xs text-fg-4">
          <div class="mb-2">{{ auth.user?.username }}</div>
          <div class="flex items-center justify-between">
            <button class="hover:text-danger-on-dark" @click="handleLogout">Log out</button>
            <label class="relative cursor-pointer">
              <input
                type="checkbox"
                id="dark-toggle"
                class="sr-only peer"
                v-model="dataScheme"
                true-value="dark"
                false-value="light"
              />
              <div class="w-9 h-5 bg-fg-1 rounded-full peer-checked:bg-accent duration-200"></div>
              <div class="absolute left-0.5 top-0.5 bg-white w-4 h-4 rounded-full duration-200 peer-checked:translate-x-full flex items-center justify-center text-line-on-dark peer-checked:[&>.sun]:hidden peer-checked:[&>.moon]:block">
                <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="2" stroke="currentColor" class="size-2.5 sun block">
                  <path stroke-linecap="round" stroke-linejoin="round" d="M12 3v2.25m6.364.386-1.591 1.591M21 12h-2.25m-.386 6.364-1.591-1.591M12 18.75V21m-4.773-4.227-1.591 1.591M5.25 12H3m4.227-4.773L5.636 5.636M15.75 12a3.75 3.75 0 1 1-7.5 0 3.75 3.75 0 0 1 7.5 0Z" />
                </svg>
                <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" class="size-2.5 moon hidden">
                  <path stroke-linecap="round" stroke-linejoin="round" d="M21.752 15.002A9.72 9.72 0 0 1 18 15.75c-5.385 0-9.75-4.365-9.75-9.75 0-1.33.266-2.597.748-3.752A9.753 9.753 0 0 0 3 11.25C3 16.635 7.365 21 12.75 21a9.753 9.753 0 0 0 9.002-5.998Z" />
                </svg>
              </div>
            </label>
          </div>
        </div>
      </aside>
      <main class="flex-1 p-4 md:p-6 overflow-auto">
        <router-view />
      </main>
    </div>
    <div v-else class="min-h-screen flex items-center justify-center p-4">
      <router-view />
    </div>
  </div>
</template>

<script lang="ts">
const nav = [
  { to: '/pages', label: 'Pages' },
  { to: '/tags', label: 'Tags' },
  { to: '/files', label: 'Files' },
  { to: '/galleries', label: 'Galleries' },
  { to: '/menu', label: 'Menu' },
  { to: '/design', label: 'Design' },
  { to: '/tokens', label: 'Tokens' },
  { to: '/users', label: 'Users' },
  { to: '/assistant', label: 'Assistant' },
  { to: '/providers', label: 'LLM providers' },
  { to: '/models', label: 'LLM models' },
  { to: '/tool-permissions', label: 'Tool permissions' },
  { to: '/mcp-servers', label: 'MCP servers' },
]
export default { name: 'App' }
</script>
