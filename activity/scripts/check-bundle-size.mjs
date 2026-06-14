// Fails when the gzipped initial JS chunk exceeds the budget. The gallery
// runs in a sandboxed iframe on phones; a runaway bundle is a regression.
// Phase 15 budget: 50KB gzipped (tightened in Phase 19).

import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { gzipSync } from 'node:zlib';

const BUDGET_BYTES = 50 * 1024;
const ASSET_DIR = 'dist/assets';

let files;
try {
  files = readdirSync(ASSET_DIR).filter((f) => f.endsWith('.js'));
} catch {
  console.error(`✗ no build found in ${ASSET_DIR} — run "npm run build" first`);
  process.exit(1);
}

const sizeOf = (f) => readFileSync(join(ASSET_DIR, f)).length;

// Vite names the entry chunk `index-<hash>.js`; fall back to the largest.
const entry =
  files.find((f) => /^index-.*\.js$/.test(f)) ?? files.sort((a, b) => sizeOf(b) - sizeOf(a))[0];

if (!entry) {
  console.error(`✗ no JS chunks in ${ASSET_DIR}`);
  process.exit(1);
}

const gzipped = gzipSync(readFileSync(join(ASSET_DIR, entry))).length;
const kb = (gzipped / 1024).toFixed(1);
const budgetKb = (BUDGET_BYTES / 1024).toFixed(0);

if (gzipped > BUDGET_BYTES) {
  console.error(`✗ initial chunk ${entry} is ${kb}KB gzipped — over the ${budgetKb}KB budget`);
  process.exit(1);
}

console.log(`✓ initial chunk ${entry} is ${kb}KB gzipped (budget ${budgetKb}KB)`);
