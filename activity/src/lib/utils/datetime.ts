// Date formatting via the platform Intl API — no date library (see
// docs/svelte-guidelines.md, dependency discipline).

function shortDate(locale: string | undefined, timeZone: string | undefined): Intl.DateTimeFormat {
  const options: Intl.DateTimeFormatOptions = { year: 'numeric', month: 'short', day: 'numeric' };
  if (timeZone) {
    try {
      return new Intl.DateTimeFormat(locale, { ...options, timeZone });
    } catch {
      // A zone name this browser doesn't know: the device's zone is the best
      // that is left.
    }
  }
  return new Intl.DateTimeFormat(locale, options);
}

/**
 * Formats an archive timestamp (unix seconds) as e.g. "Nov 14, 2023". Pass
 * the server's `timeZone` so the date matches the calendar cell the day sits
 * in; without one the device's zone is used.
 */
export function formatPostedAt(unixSeconds: number, locale?: string, timeZone?: string): string {
  const date = new Date(unixSeconds * 1000);
  if (Number.isNaN(date.getTime())) return '';
  return shortDate(locale, timeZone).format(date);
}

/** The instant as a machine-readable value for `<time datetime>`, or `undefined`. */
export function isoInstant(unixSeconds: number): string | undefined {
  const date = new Date(unixSeconds * 1000);
  return Number.isNaN(date.getTime()) ? undefined : date.toISOString();
}
