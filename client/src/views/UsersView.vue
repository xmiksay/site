<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useUsersStore } from '../stores/users'
import { ApiError } from '../api'

const users = useUsersStore()
onMounted(users.load)

const newUsername = ref('')
const newPassword = ref('')
const error = ref<string | null>(null)

async function add() {
  error.value = null
  const username = newUsername.value.trim()
  if (!username || !newPassword.value) return
  try {
    await users.create(username, newPassword.value)
    newUsername.value = ''
    newPassword.value = ''
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : 'Failed to create user'
  }
}

async function changePassword(id: number, username: string) {
  const password = prompt(`New password for "${username}"`)
  if (!password) return
  try {
    await users.changePassword(id, password)
    alert(`Password changed for "${username}".`)
  } catch (e) {
    alert(e instanceof ApiError ? e.message : 'Failed to change password')
  }
}

async function remove(id: number, username: string) {
  if (
    !confirm(
      `Delete user "${username}"? Their pages, files, and galleries will be reassigned to you.`,
    )
  )
    return
  try {
    await users.remove(id)
  } catch (e) {
    alert(e instanceof ApiError ? e.message : 'Failed to delete user')
  }
}
</script>

<template>
  <div class="space-y-4">
    <h1 class="text-xl font-semibold">Users</h1>

    <form class="bg-surface shadow rounded p-3 flex gap-2 items-end" @submit.prevent="add">
      <label class="flex-1">
        <span class="text-xs text-fg-3">Username</span>
        <input v-model="newUsername" class="mt-1 w-full rounded border border-line-1 px-2 py-1.5" />
      </label>
      <label class="flex-1">
        <span class="text-xs text-fg-3">Password</span>
        <input
          v-model="newPassword"
          type="password"
          autocomplete="new-password"
          class="mt-1 w-full rounded border border-line-1 px-2 py-1.5"
        />
      </label>
      <button class="rounded button-primary px-3 py-1.5 text-sm">
        Create
      </button>
    </form>

    <div v-if="error" class="bg-danger-bg border border-danger-soft text-danger-strong rounded p-2 text-sm">
      {{ error }}
    </div>

    <div class="bg-surface rounded-lg shadow overflow-x-auto">
      <table class="min-w-full text-sm">
        <thead class="bg-surface-raised text-fg-2">
          <tr>
            <th class="text-left px-4 py-2">Username</th>
            <th class="px-4 py-2"></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="u in users.items" :key="u.id" class="border-t border-line-3">
            <td class="px-4 py-2">
              {{ u.username }}
              <span v-if="u.is_self" class="ml-2 text-xs text-fg-4">(you)</span>
            </td>
            <td class="px-4 py-2 text-right space-x-3">
              <button
                class="text-accent hover:underline"
                @click="changePassword(u.id, u.username)"
              >
                Change password
              </button>
              <button
                v-if="!u.is_self"
                class="text-danger hover:underline"
                @click="remove(u.id, u.username)"
              >
                Delete
              </button>
            </td>
          </tr>
          <tr v-if="users.items.length === 0">
            <td colspan="2" class="px-4 py-6 text-center text-fg-4">No users.</td>
          </tr>
        </tbody>
      </table>
    </div>
  </div>
</template>
