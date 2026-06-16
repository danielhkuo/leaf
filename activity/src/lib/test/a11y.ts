import * as axe from 'axe-core';
import { expect } from 'vitest';

/**
 * Runs axe-core against a rendered container and asserts no violations.
 * `color-contrast` is disabled because it needs layout, which jsdom does not
 * compute — contrast is verified in the manual device pass instead.
 */
export async function expectNoA11yViolations(container: Element): Promise<void> {
  const results = await axe.run(container, {
    rules: { 'color-contrast': { enabled: false } },
  });
  const violations = results.violations.map((v) => `${v.id}: ${v.help} (${v.nodes.length} node/s)`);
  expect(violations).toStrictEqual([]);
}
