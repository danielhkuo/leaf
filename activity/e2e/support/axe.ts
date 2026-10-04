// axe-core in a real browser. The component tests run the same rule set in
// jsdom (src/lib/test/a11y.ts) with `color-contrast` turned off, because
// jsdom computes no layout. Here it is on: this is the run that sees real
// colours on real backgrounds. No other rule is disabled in either place.
//
// axe sorts what it finds into violations and "incomplete": elements it
// could not judge. For contrast that is anything it cannot name one
// background colour for. An element nobody judged is a check that did not
// run, so it is reported here like a violation, unless UNJUDGED names it.

import { createRequire } from 'node:module';

import type { Page } from '@playwright/test';
import type { AxeResults, ElementContext, RunOptions } from 'axe-core';

const AXE_SCRIPT = createRequire(import.meta.url).resolve('axe-core/axe.min.js');

interface AxeGlobal {
  run(context: ElementContext, options: RunOptions): Promise<AxeResults>;
}

/**
 * Something axe cannot judge and nothing else here can either, and why. A
 * new one needs the same: the rule, axe's own reason (the `messageKey` the
 * report prints) and the elements, as narrowly as they can be named.
 */
interface Unjudged {
  name: string;
  rule: string;
  reason: string;
  selector: string;
}

const UNJUDGED: readonly Unjudged[] = [
  {
    // The date, the day number and "+1" are white text on the day's picture,
    // over a scrim that DayCell.svelte draws as a pseudo-element. What is
    // behind them is a photo, so there is no one ratio to compute; the scrim
    // is what keeps them legible on any picture.
    name: 'the labels on a calendar day’s picture',
    rule: 'color-contrast',
    reason: 'pseudoContent',
    selector: 'button[data-days] span',
  },
  {
    // axe compares what lies under each line of a text, all the way down the
    // page, and gives up when the lines differ. Under the open day viewer
    // that includes the page it covers, which is inert and cannot be seen:
    // a message that wraps there has different covered things under each
    // line. Something drawn over the text is another reason (`bgOverlap`)
    // and is still reported.
    name: 'text that wraps in the open day viewer',
    rule: 'color-contrast',
    reason: 'elmPartiallyObscuring',
    selector: '[aria-modal="true"] *',
  },
  {
    // A day's video is a member's own clip, kept as it was posted. leaf has
    // no captions for it, and axe cannot tell whether it needs any: it only
    // asks for someone to check. What the post said is under the video, and
    // the original post is one button away.
    name: 'whether a day’s video needs captions',
    rule: 'video-caption',
    reason: 'caption',
    selector: '[aria-modal="true"] video',
  },
];

/**
 * One line per violation, and per rule with elements axe could not judge,
 * naming the elements and what axe measured or why it could not.
 *
 * One thing is done to the page for the scan and undone after it. app.css
 * draws a select's chevron as two small gradients in its right padding, and
 * axe judges no text that has a gradient anywhere behind its element. The
 * chevron is nowhere near the text, so it is taken off, and the text is
 * judged against the colour that really is behind it. Only such a decoration
 * is: layers that do not repeat and are no wider than the padding they sit
 * in. A select with any other background image comes back as not judged.
 */
export async function axeViolations(page: Page): Promise<string[]> {
  // Once per page: a test may scan, change the page and scan again.
  const loaded = await page.evaluate(() => 'axe' in window);
  if (!loaded) await page.addScriptTag({ path: AXE_SCRIPT });
  return page.evaluate(
    async (exceptions) => {
      const selects = [...document.querySelectorAll('select')].filter((select) => {
        const style = getComputedStyle(select);
        if (style.backgroundImage === 'none') return false;
        // Each layer's width, in px; a percentage or `auto` is no small decoration.
        const widths = style.backgroundSize
          .split(',')
          .map((size) => Number(/^\s*([\d.]+)px/.exec(size)?.[1]));
        const repeats = style.backgroundRepeat.split(/[\s,]+/);
        return (
          repeats.every((repeat) => repeat === 'no-repeat') &&
          widths.every((width) => width <= Number.parseFloat(style.paddingRight))
        );
      });
      const before = selects.map((select) => select.getAttribute('style'));
      for (const select of selects)
        select.style.setProperty('background-image', 'none', 'important');

      let results: AxeResults;
      try {
        results = await (window as unknown as { axe: AxeGlobal }).axe.run(document, {
          elementRef: true,
        });
      } finally {
        selects.forEach((select, i) => {
          const style = before[i];
          if (typeof style === 'string') select.setAttribute('style', style);
          else select.removeAttribute('style');
        });
      }

      const lines: string[] = [];
      for (const violation of results.violations) {
        const nodes = violation.nodes.map((node) => {
          const measured = node.any.map((check) => check.message).join(' ');
          return `${node.target.join(' ')}${measured ? ` (${measured})` : ''}`;
        });
        lines.push(`${violation.id}: ${violation.help}\n  ${nodes.join('\n  ')}`);
      }
      for (const rule of results.incomplete) {
        const nodes = rule.nodes.flatMap((node) => {
          const checks = [...node.any, ...node.all, ...node.none];
          const reasons = checks.map((check) => {
            const data: unknown = check.data;
            const key = (data as { messageKey?: unknown } | null)?.messageKey;
            return typeof key === 'string' ? key : check.id;
          });
          const excepted = exceptions.some(
            (e) =>
              e.rule === rule.id &&
              reasons.includes(e.reason) &&
              node.element?.matches(e.selector) === true,
          );
          if (excepted) return [];
          const why = checks.map((check) => check.message).join(' ');
          return [`${node.target.join(' ')} (${why} Reason: ${reasons.join(', ')})`];
        });
        if (nodes.length > 0)
          lines.push(`${rule.id}, not judged: ${rule.help}\n  ${nodes.join('\n  ')}`);
      }
      return lines;
    },
    [...UNJUDGED],
  );
}
