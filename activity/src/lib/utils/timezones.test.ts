import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  deviceTimezone,
  hasTimezoneList,
  isKnownTimezone,
  timezoneOffsetLabel,
  timezoneOptions,
} from './timezones';

afterEach(() => {
  vi.restoreAllMocks();
});

describe('timezoneOptions', () => {
  it('lists the browser’s zones plus UTC, sorted and without repeats', () => {
    const zones = timezoneOptions();
    expect(zones).toContain('UTC');
    expect(zones).toContain('America/Chicago');
    expect(new Set(zones).size).toBe(zones.length);
    expect(zones).toEqual([...zones].sort((a, b) => a.localeCompare(b, 'en')));
  });

  it('always includes the stored value, even one the browser does not list', () => {
    expect(timezoneOptions('Asia/Calcutta')).toContain('Asia/Calcutta');
    expect(timezoneOptions('Not/AZone')).toContain('Not/AZone');
    expect(timezoneOptions(null)).not.toContain('');
    expect(timezoneOptions('')).not.toContain('');
  });

  it('includes the device zone', () => {
    const device = deviceTimezone();
    expect(device).toBeTruthy();
    expect(timezoneOptions()).toContain(device);
  });

  it('degrades to a short list where the webview cannot enumerate zones', () => {
    vi.spyOn(Intl, 'supportedValuesOf').mockImplementation(() => {
      throw new RangeError('unsupported');
    });
    const zones = timezoneOptions('Europe/Paris');
    expect(zones).toContain('UTC');
    expect(zones).toContain('Europe/Paris');
    expect(zones.length).toBeLessThanOrEqual(3);
  });
});

describe('hasTimezoneList', () => {
  it('reports whether Intl can enumerate zones', () => {
    expect(hasTimezoneList()).toBe(true);
  });
});

describe('isKnownTimezone', () => {
  it('accepts IANA names and aliases, rejects the rest', () => {
    expect(isKnownTimezone('America/Chicago')).toBe(true);
    expect(isKnownTimezone('UTC')).toBe(true);
    expect(isKnownTimezone('Asia/Calcutta')).toBe(true);
    expect(isKnownTimezone('Chicago')).toBe(false);
    expect(isKnownTimezone('')).toBe(false);
    // Some webviews accept a bare offset; leaf's server does not.
    expect(isKnownTimezone('+05:00')).toBe(false);
  });
});

describe('timezoneOffsetLabel', () => {
  it('shows the offset in force on the given date', () => {
    expect(timezoneOffsetLabel('America/Chicago', new Date('2026-01-15T12:00:00Z'))).toBe('UTC-6');
    expect(timezoneOffsetLabel('America/Chicago', new Date('2026-07-15T12:00:00Z'))).toBe('UTC-5');
    expect(timezoneOffsetLabel('Asia/Kolkata', new Date('2026-01-15T12:00:00Z'))).toBe('UTC+5:30');
    expect(timezoneOffsetLabel('UTC', new Date('2026-01-15T12:00:00Z'))).toBe('UTC');
  });

  it('is empty for a zone the webview does not know', () => {
    expect(timezoneOffsetLabel('Not/AZone')).toBe('');
  });
});
