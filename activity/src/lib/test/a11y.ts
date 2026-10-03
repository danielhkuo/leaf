import * as axe from 'axe-core';
import { expect } from 'vitest';

/**
 * Runs axe-core against a rendered container and asserts no violations.
 * `color-contrast` is disabled because it needs layout, which jsdom does not
 * compute. Contrast is guarded at the token level instead: see
 * `utils/contrast.test.ts`, which checks every text and control colour in
 * app.css against the surfaces it is used on.
 */
export async function expectNoA11yViolations(container: Element): Promise<void> {
  const results = await axe.run(container, {
    rules: { 'color-contrast': { enabled: false } },
  });
  const violations = results.violations.map((v) => `${v.id}: ${v.help} (${v.nodes.length} node/s)`);
  expect(violations).toStrictEqual([]);
}
