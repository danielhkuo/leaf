// The Vite server the mock suites run against: the app's own config, served
// on the suite's port. It differs from `npm run dev` in three ways, each so
// that a run is repeatable and cannot disturb anything else:
//
// - no proxy: nothing a test does can reach a leaf-server on this machine
//   (the mock screens answer `/api` inside the page);
// - no hot reload: a file saved mid-run does not reload a page under test;
// - its own dependency cache, so it never rewrites the one a running
//   `npm run dev` is serving from.
import { defineConfig } from 'vite';

import base from '../../vite.config';
import { MOCK_PORT } from './env';

export default defineConfig({
  ...base,
  cacheDir: 'node_modules/.vite-e2e',
  server: {
    host: '127.0.0.1',
    port: MOCK_PORT,
    strictPort: true,
    hmr: false,
  },
});
