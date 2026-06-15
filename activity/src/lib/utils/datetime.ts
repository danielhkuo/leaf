// Date formatting via the platform Intl API — no date library (see
// docs/svelte-guidelines.md, dependency discipline).

/** Formats an archive timestamp (unix seconds) as e.g. "Nov 14, 2023". */
export function formatPostedAt(unixSeconds: number, locale?: string): string {
  return new Intl.DateTimeFormat(locale, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  }).format(new Date(unixSeconds * 1000));
}
