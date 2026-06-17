import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vite';

// In dev the API is proxied to a locally-running leaf-server so the app is
// same-origin (matching production behind Discord's proxy). Override the
// target with LEAF_API_TARGET if leaf-server runs elsewhere.
const API_TARGET = process.env.LEAF_API_TARGET ?? 'http://localhost:3777';

// Discord loads the activity over a public HTTPS tunnel, so the dev server is
// reached via a non-localhost Host header — which Vite blocks by default since
// 5.4. Allow quick tunnels (*.trycloudflare.com) out of the box; set
// LEAF_DEV_HOST=leaf-dev.example.com for a stable named tunnel.
const devHost = process.env.LEAF_DEV_HOST;

export default defineConfig({
  plugins: [svelte()],
  server: {
    port: 5173,
    allowedHosts: devHost ? [devHost, '.trycloudflare.com'] : ['.trycloudflare.com'],
    proxy: {
      '/api': { target: API_TARGET, changeOrigin: true },
      // The admin panel's OAuth login/callback are server routes (not the
      // SPA), so forward them to leaf-server — Vite serves /admin itself.
      '/admin/login': { target: API_TARGET, changeOrigin: true },
      '/admin/callback': { target: API_TARGET, changeOrigin: true },
    },
  },
  build: {
    target: 'es2022',
    sourcemap: false,
  },
});
