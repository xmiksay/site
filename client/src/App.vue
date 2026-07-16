<script setup lang="ts">
import { ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useAuthStore } from './stores/auth'
import { useWsStore } from './stores/ws'

const auth = useAuthStore()
const ws = useWsStore()
const router = useRouter()

const mobileOpen = ref(false)

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

async function handleLogout() {
  await auth.logout()
  router.push('/login')
}
</script>

<template>
  <div class="min-h-screen bg-gray-50 text-gray-900">
    <div v-if="auth.isLoggedIn" class="md:flex md:min-h-screen">
      <header
        class="md:hidden flex items-center justify-between bg-gray-800 text-gray-100 px-4 py-3"
      >
        <a href="/" class="font-semibold">Site</a>
        <button
          type="button"
          class="p-2 rounded hover:bg-gray-700 focus:outline-none focus:ring-2 focus:ring-gray-500"
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
        class="md:hidden fixed inset-0 z-30 bg-black/40"
        @click="mobileOpen = false"
      ></div>

      <aside
        class="bg-gray-800 text-gray-100 flex flex-col z-40
               fixed inset-y-0 left-0 w-64 transform transition-transform duration-200 ease-out
               md:static md:w-56 md:translate-x-0"
        :class="mobileOpen ? 'translate-x-0' : '-translate-x-full md:translate-x-0'"
      >
        <div class="px-4 py-4 border-b border-gray-700 font-semibold flex items-center justify-between">
          <a href="/">Site</a>
          <button
            type="button"
            class="md:hidden p-1 rounded hover:bg-gray-700"
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
            class="block px-3 py-2 rounded hover:bg-gray-700"
            active-class="bg-gray-700 font-medium"
          >
            {{ item.label }}
          </router-link>
        </nav>
        <div class="px-4 py-3 border-t border-gray-700 text-xs text-gray-400">
          <div class="mb-2">{{ auth.user?.username }}</div>
          <button class="hover:text-red-400" @click="handleLogout">Log out</button>
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
