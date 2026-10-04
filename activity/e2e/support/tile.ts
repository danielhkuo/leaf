// Measurements for leaf minimised: Discord shrinks the Activity to a small
// tile, and leaf shows one card in it (Minimisable.svelte). The card has to
// be all there is: nothing of it past the tile's edges, no word cut off,
// and nothing behind it that a tap, a Tab or a screen reader could reach.

import type { Page } from '@playwright/test';

/** The card: the page's one `<main>` while leaf is a tile. */
export const CARD = 'main.tile';

export interface TileReport {
  /** How far the page can scroll each way: nowhere, when the card is all there is. */
  scroll: { x: number; y: number };
  /** Parts of the card that reach past an edge of the tile. */
  outside: string[];
  /** Text in the card that is cut off: more of it than the box it is drawn in holds. */
  cutOff: string[];
  /**
   * Words outside the square in the middle of the page. Discord shows only
   * that square when the page is not one itself (120 x 214 behind a
   * 120 x 120 tile on Android), so words outside it are never seen.
   */
  cropped: string[];
  /**
   * What is behind the card and should not be: something with a box, or a
   * control that is not inert.
   */
  behind: string[];
  /** Points of the tile where something other than the card is on top. */
  uncovered: string[];
  /** How many pieces of the card were measured: 0 would mean a blind check. */
  parts: number;
}

export function auditTile(page: Page): Promise<TileReport> {
  return page.evaluate((selector) => {
    // Half a pixel of slack: sizes come back as fractions at 2x and 3x.
    const SLACK = 0.5;
    const width = window.innerWidth;
    const height = window.innerHeight;

    function describe(el: Element): string {
      const classes = [...el.classList].filter((c) => !c.startsWith('svelte-'));
      const text = (el.textContent ?? '').replace(/\s+/g, ' ').trim().slice(0, 40);
      return `${el.tagName.toLowerCase()}${classes.map((c) => `.${c}`).join('')}${text ? ` "${text}"` : ''}`;
    }

    const card = document.querySelector(selector);
    const parts = card ? [card, ...card.querySelectorAll('*')] : [];

    const side = Math.min(width, height);
    const shown = {
      left: (width - side) / 2,
      top: (height - side) / 2,
      right: (width + side) / 2,
      bottom: (height + side) / 2,
    };

    const outside: string[] = [];
    const cutOff: string[] = [];
    const cropped: string[] = [];
    for (const el of parts) {
      const rect = el.getBoundingClientRect();
      if (rect.width === 0 && rect.height === 0) continue;
      if (
        rect.left < -SLACK ||
        rect.top < -SLACK ||
        rect.right > width + SLACK ||
        rect.bottom > height + SLACK
      ) {
        outside.push(
          `${describe(el)} spans ${rect.left.toFixed(1)},${rect.top.toFixed(1)} to ${rect.right.toFixed(1)},${rect.bottom.toFixed(1)}`,
        );
      }
      // A box that holds text itself, and less of it than there is.
      const holdsText = [...el.childNodes].some(
        (node) => node.nodeType === Node.TEXT_NODE && (node.textContent ?? '').trim() !== '',
      );
      if (!holdsText || getComputedStyle(el).display === 'inline') continue;
      if (
        rect.left < shown.left - SLACK ||
        rect.top < shown.top - SLACK ||
        rect.right > shown.right + SLACK ||
        rect.bottom > shown.bottom + SLACK
      ) {
        cropped.push(
          `${describe(el)} spans ${rect.left.toFixed(1)},${rect.top.toFixed(1)} to ${rect.right.toFixed(1)},${rect.bottom.toFixed(1)}`,
        );
      }
      if (el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1) {
        cutOff.push(
          `${describe(el)} needs ${el.scrollWidth} x ${el.scrollHeight}, has ${el.clientWidth} x ${el.clientHeight}`,
        );
      }
    }

    const CONTROLS =
      'a[href], button, select, textarea, summary, video, input:not([type="hidden"]), ' +
      '[tabindex], [role="button"], [role="link"]';
    const behind: string[] = [];
    for (const el of document.body.querySelectorAll('*')) {
      if (card?.contains(el) || el.contains(card)) continue;
      if (el.getClientRects().length > 0) behind.push(`${describe(el)} has a box`);
      else if (el.matches(CONTROLS) && !el.closest('[inert]')) {
        behind.push(`${describe(el)} is not inert`);
      }
    }

    // Nine points across the tile, corners included (just inside them).
    const uncovered: string[] = [];
    for (const fx of [0.02, 0.5, 0.98]) {
      for (const fy of [0.02, 0.5, 0.98]) {
        const top = document.elementFromPoint(width * fx, height * fy);
        if (!top || !card?.contains(top)) {
          uncovered.push(
            `${Math.round(width * fx)},${Math.round(height * fy)}: ${top ? describe(top) : 'nothing'}`,
          );
        }
      }
    }

    const root = document.documentElement;
    return {
      scroll: {
        x: Math.max(root.scrollWidth, document.body.scrollWidth) - root.clientWidth,
        y: Math.max(root.scrollHeight, document.body.scrollHeight) - root.clientHeight,
      },
      outside,
      cutOff,
      cropped,
      behind,
      uncovered,
      parts: parts.length,
    };
  }, CARD);
}
