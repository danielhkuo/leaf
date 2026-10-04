// Where an app spec reaches into the gallery by something other than a role,
// a label or its text. The specs find what a person or a screen reader finds
// wherever the page offers it; these are the few places it does not, kept in
// one file so that a renamed class or attribute is one line to change.

import type { FrameLocator, Locator } from '@playwright/test';

/** The calendar cell holding a day (`data-days` lists the days posted on its date). */
export function cell(app: FrameLocator, day: number): Locator {
  return app.locator(`[data-days~="${day}"]`);
}

/** One month of the calendar, by its `YYYY-MM` key. */
export function month(app: FrameLocator, key: string): Locator {
  return app.locator(`[data-month="${key}"]`);
}

/**
 * The names on the series list's cards, in order. A card's own text runs its
 * name, description and day count together, and the revoked card is not a
 * button, so the name alone is only to be had by its class (SeriesCard.svelte).
 */
export function seriesNames(app: FrameLocator): Locator {
  return app.getByRole('listitem').locator('.name');
}

/**
 * What a calendar cell shows only by how it is drawn (DayCell.svelte). An
 * empty date is hidden from screen readers and holds nothing but its number,
 * so these classes are all that tells one kind from another.
 */
export const DRAWN = {
  /** The hatched tile of a day with no picture to show. */
  noPicture: '.missing',
  /** The date the guild's calendar is on. */
  today: '.today',
  /** A date before the first post or after today: no post could be missing there. */
  outsideTheSeries: '.cell.outside',
} as const;
