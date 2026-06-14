import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';

// In dev the API is proxied to a locally-running leaf-server so the app is
// same-origin (matching production behind Discord's proxy). Override the
// target with LEAF_API_TARGET if leaf-server runs elsewhere.
const API_TARGET = process.env.LEAF_API_TARGET ?? 'http://localhost:8080';

export default defineConfig({
  plugins: [svelte()],
  server: {
    port: 5173,
    proxy: {
      '/api': { target: API_TARGET, changeOrigin: true },
    },
  },
  build: {
    target: 'es2022',
    sourcemap: false,
  },
});
