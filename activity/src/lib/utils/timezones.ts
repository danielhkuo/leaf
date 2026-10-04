// IANA timezone choices for the reminder and server timezone selects. Discord
// exposes no user timezone, so the device's own zone is the best default the
// Activity or the admin panel can offer. Pure `Intl`, no dependencies: both
// lazy chunks (creator views, admin panel) import this.

/** The device's IANA zone (e.g. `America/Chicago`), or `null` when the webview won't say. */
export function deviceTimezone(): string | null {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || null;
  } catch {
    return null;
  }
}

/** Whether this webview can list every zone (Safari 15.4+, Chrome 99+). */
export function hasTimezoneList(): boolean {
  return typeof Intl.supportedValuesOf === 'function';
}

/**
 * Zones for a `<select>`, sorted. Always holds `UTC` (browsers leave it out
 * of their list), the device zone, and `current` (the stored value, which may
 * be an alias the browser's list does not carry), so the control can show
 * what is saved. Where {@link hasTimezoneList} is false the result is only
 * those few: offer a text input next to it.
 */
export function timezoneOptions(current?: string | null): string[] {
  const zones = new Set<string>(['UTC']);
  if (hasTimezoneList()) {
    try {
      for (const tz of Intl.supportedValuesOf('timeZone')) zones.add(tz);
    } catch {
      /* fall through with the short list */
    }
  }
  const device = deviceTimezone();
  if (device) zones.add(device);
  if (current) zones.add(current);
  return [...zones].sort((a, b) => a.localeCompare(b, 'en'));
}

/**
 * Whether the webview accepts `tz` as a zone name (case-insensitive, aliases
 * included). A bare offset such as `+05:00` is refused even where the webview
 * takes it: the server only knows named zones.
 */
export function isKnownTimezone(tz: string): boolean {
  if (!/^[A-Za-z]/.test(tz)) return false;
  try {
    new Intl.DateTimeFormat('en-US', { timeZone: tz });
    return true;
  } catch {
    return false;
  }
}

/**
 * The zone's UTC offset at `at`, e.g. `UTC-5`, `UTC+5:30`, `UTC`. Empty when
 * the zone is unknown or the webview can't format offsets, so it can be
 * appended to a label unconditionally.
 */
export function timezoneOffsetLabel(tz: string, at: Date = new Date()): string {
  try {
    const parts = new Intl.DateTimeFormat('en-US', {
      timeZone: tz,
      timeZoneName: 'shortOffset',
    }).formatToParts(at);
    const name = parts.find((p) => p.type === 'timeZoneName')?.value ?? '';
    // Engines disagree on a zero offset ("GMT" or "GMT+0"); show plain "UTC".
    return name.replace(/^GMT/, 'UTC').replace(/^UTC[+-]0?0(:00)?$/, 'UTC');
  } catch {
    return '';
  }
}
