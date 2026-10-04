// Runs once before the mock projects. The first request for a module makes
// Vite compile it, and the first for a dependency can make Vite rebuild its
// cache and reload the page. Doing both here, on one page, keeps that out of
// the parallel tests, and says so plainly when the mock does not start.

import { test } from '../support/mock';

// Between them these load every chunk: the gallery, the viewer, the creator
// screens and the admin panel.
const CHUNKS = ['viewer', 'settings', 'admin-panel', 'landing'] as const;

test('the mock screens load', async ({ mock, guard }) => {
  test.setTimeout(120_000);
  // Only loading is checked here. What a screen logs is for its own tests to
  // report, screen by screen, which they cannot do if this one fails first.
  guard.allow(/./);
  for (const id of CHUNKS) await mock.open(id);
});
