// The measurements the phone-width pass used to make by eye: nothing wider
// than the screen, every control big enough to tap, no field small enough to
// make iOS zoom. One pass over the page's elements, run inside the page.

import type { Page } from '@playwright/test';

/** Apple's minimum tap target, and leaf's `--touch-target`. */
export const MIN_TARGET = 44;
/** Below this, iOS zooms the page into a field on focus and stays zoomed. */
export const MIN_FIELD_FONT = 16;
/** A calendar cell with something archived: the button that opens its day (DayCell.svelte). */
export const CALENDAR_DAY = 'button[data-days]';

/**
 * A control that may be smaller than MIN_TARGET, and why. Each is something
 * the code itself documents; a new one needs the same.
 */
interface TargetException {
  name: string;
  /** The controls it covers. */
  selector: string;
  /** Applies only in viewports narrower than this. */
  belowWidth?: number;
  /** The size such a control must still reach, in each direction. */
  min: number;
}

const TARGET_EXCEPTIONS: readonly TargetException[] = [
  {
    // Seven day columns and their gaps do not fit 7 x 44px in a viewport
    // narrower than 345px. Home.svelte and MonthGrid.svelte narrow the page
    // padding and the gaps there to get as close as they can: 41.7px at 320.
    name: 'calendar day cells below 345px',
    selector: CALENDAR_DAY,
    belowWidth: 345,
    min: 41,
  },
];

export interface LayoutReport {
  /** `scrollWidth - clientWidth` of the page: 0 when nothing overflows. */
  pageOverflow: number;
  /**
   * Elements that reach past the left or right edge of the viewport. Only
   * what a container scrolls sideways on purpose is left out: content that a
   * clipping container cuts off at the edge is listed, as cut off.
   */
  outside: string[];
  /** Controls smaller than their minimum tap target. */
  smallTargets: string[];
  /** Fields whose text is under MIN_FIELD_FONT. */
  smallFonts: string[];
  /** How many controls and fields were measured: 0 would mean a blind check. */
  targets: number;
  fields: number;
}

export function auditLayout(page: Page): Promise<LayoutReport> {
  return page.evaluate(
    ({ minTarget, minFont, exceptions }) => {
      // Half a pixel of slack: sizes come back as fractions at 2x and 3x.
      const SLACK = 0.5;
      const viewportWidth = document.documentElement.clientWidth;

      /** `button.link "Manage my series"`: enough to find the element. */
      function describe(el: Element): string {
        const classes = [...el.classList].filter((c) => !c.startsWith('svelte-'));
        const name = el.getAttribute('aria-label') ?? el.textContent ?? '';
        const text = name.replace(/\s+/g, ' ').trim().slice(0, 40);
        const id = el.id ? `#${el.id}` : '';
        return `${el.tagName.toLowerCase()}${id}${classes.map((c) => `.${c}`).join('')}${text ? ` "${text}"` : ''}`;
      }

      /**
       * Has a box. What a closed <details> holds counts: browsers lay it
       * out and only skip painting it, so a disclosure's controls are
       * measured here as they will be once it is opened.
       */
      function rendered(el: Element): boolean {
        const rect = el.getBoundingClientRect();
        if (rect.width === 0 || rect.height === 0) return false;
        return getComputedStyle(el).visibility !== 'hidden';
      }

      /**
       * The nearest ancestor that keeps the element from widening the page,
       * and how: `auto` and `scroll` are a container scrolled sideways on
       * purpose, `hidden` and `clip` cut the element off.
       */
      function heldBy(el: Element): { container: Element; scrolls: boolean } | null {
        for (let up = el.parentElement; up && up !== document.body; up = up.parentElement) {
          const overflow = getComputedStyle(up).overflowX;
          if (overflow !== 'visible') {
            return { container: up, scrolls: overflow === 'auto' || overflow === 'scroll' };
          }
        }
        return null;
      }

      const outside: string[] = [];
      for (const el of document.body.querySelectorAll('*')) {
        if (!rendered(el)) continue;
        const rect = el.getBoundingClientRect();
        if (rect.right <= viewportWidth + SLACK && rect.left >= -SLACK) continue;
        const held = heldBy(el);
        if (held?.scrolls) continue;
        const span = `spans ${rect.left.toFixed(1)} to ${rect.right.toFixed(1)}`;
        const cut = held ? `, cut off by ${describe(held.container)}` : '';
        outside.push(`${describe(el)} ${span}${cut}`);
      }

      /**
       * What a tap can land on. A native checkbox or radio is tapped through
       * its label ("the row around them supplies the 44px", app.css).
       */
      function targetRect(el: Element): { width: number; height: number } {
        const own = el.getBoundingClientRect();
        if (!(el instanceof HTMLInputElement) || !['checkbox', 'radio'].includes(el.type)) {
          return own;
        }
        const label = el.labels?.[0]?.getBoundingClientRect();
        if (!label) return own;
        return {
          width: Math.max(own.right, label.right) - Math.min(own.left, label.left),
          height: Math.max(own.bottom, label.bottom) - Math.min(own.top, label.top),
        };
      }

      /**
       * WCAG 2.5.8's inline exception: a link inside a sentence is as tall
       * as its line of text, and making it taller would break the paragraph.
       */
      function inlineLink(el: Element): boolean {
        if (el.tagName !== 'A' || getComputedStyle(el).display !== 'inline') return false;
        return [...(el.parentElement?.childNodes ?? [])].some(
          (node) => node.nodeType === Node.TEXT_NODE && (node.textContent ?? '').trim() !== '',
        );
      }

      const CONTROLS =
        'a[href], button, select, textarea, summary, input:not([type="hidden"]), ' +
        '[role="button"], [role="link"]';
      const smallTargets: string[] = [];
      let targets = 0;
      for (const el of document.body.querySelectorAll(CONTROLS)) {
        // Under the open day viewer the page is inert: nothing there is a target.
        if (!rendered(el) || el.closest('[inert]') || inlineLink(el)) continue;
        targets += 1;
        const exception = exceptions.find(
          (e) =>
            el.matches(e.selector) && (e.belowWidth === undefined || viewportWidth < e.belowWidth),
        );
        const min = exception?.min ?? minTarget;
        const { width, height } = targetRect(el);
        if (width >= min - SLACK && height >= min - SLACK) continue;
        const rule = exception ? `${min}px, the exception for ${exception.name}` : `${min}px`;
        smallTargets.push(
          `${describe(el)} is ${width.toFixed(1)} x ${height.toFixed(1)} (minimum ${rule})`,
        );
      }

      const FIELDS =
        'select, textarea, input:not([type="hidden"]):not([type="checkbox"]):not([type="radio"])' +
        ':not([type="button"]):not([type="submit"]):not([type="reset"]):not([type="file"])' +
        ':not([type="range"]):not([type="color"]):not([type="image"])';
      const smallFonts: string[] = [];
      let fields = 0;
      for (const el of document.body.querySelectorAll(FIELDS)) {
        if (!rendered(el)) continue;
        fields += 1;
        const size = Number.parseFloat(getComputedStyle(el).fontSize);
        if (size < minFont)
          smallFonts.push(`${describe(el)} has ${size}px text (minimum ${minFont}px)`);
      }

      const root = document.documentElement;
      return {
        pageOverflow: Math.max(root.scrollWidth, document.body.scrollWidth) - root.clientWidth,
        outside,
        smallTargets,
        smallFonts,
        targets,
        fields,
      };
    },
    { minTarget: MIN_TARGET, minFont: MIN_FIELD_FONT, exceptions: [...TARGET_EXCEPTIONS] },
  );
}
