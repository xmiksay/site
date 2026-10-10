import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  base: '/admin/',
  plugins: [vue(), tailwindcss()],
  server: {
    proxy: {
      '/api': 'http://localhost:3000',
      // Everything outside the SPA is the public site, for the Design
      // studio's preview iframe.
      '^/(?!admin(/|$)|api/)': 'http://localhost:3000',
    },
  },
})
