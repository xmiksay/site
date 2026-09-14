<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { useAuthStore } from '../stores/auth'

const auth = useAuthStore()
const router = useRouter()

const username = ref('')
const password = ref('')
const error = ref('')
const submitting = ref(false)

async function submit() {
  error.value = ''
  submitting.value = true
  try {
    await auth.login(username.value, password.value)
    router.push('/pages')
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'login failed'
  } finally {
    submitting.value = false
  }
}
</script>

<template>
  <form
    class="bg-surface shadow rounded-lg p-6 w-80 space-y-4"
    @submit.prevent="submit"
  >
    <h1 class="text-lg font-semibold">Site Admin</h1>
    <label class="block">
      <span class="text-sm text-fg-2">Username</span>
      <input
        v-model="username"
        type="text"
        autocomplete="username"
        class="mt-1 w-full rounded border border-line-1 px-2 py-1.5"
        autofocus
      />
    </label>
    <label class="block">
      <span class="text-sm text-fg-2">Password</span>
      <input
        v-model="password"
        type="password"
        autocomplete="current-password"
        class="mt-1 w-full rounded border border-line-1 px-2 py-1.5"
      />
    </label>
    <p v-if="error" class="text-sm text-danger">{{ error }}</p>
    <button
      type="submit"
      :disabled="submitting"
      class="w-full rounded button-primary py-2 disabled:opacity-50"
    >
      {{ submitting ? '…' : 'Log in' }}
    </button>
  </form>
</template>
