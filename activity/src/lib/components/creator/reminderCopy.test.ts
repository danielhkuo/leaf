import { describe, expect, it } from 'vitest';

import { deviceClock, formatClock, reminderSummary, zoneOffsetMinutes } from './reminderCopy';

/** Some ICU versions put a narrow no-break space before AM/PM. */
function plain(text: string): string {
  return text.replace(/\s/g, ' ');
}

const JANUARY = new Date('2026-01-15T12:00:00Z');
const JULY = new Date('2026-07-15T12:00:00Z');

describe('formatClock', () => {
  it('writes a time the way the locale does', () => {
    expect(plain(formatClock('20:00', 'en-US'))).toBe('8:00 PM');
    expect(plain(formatClock('09:05', 'en-US'))).toBe('9:05 AM');
    expect(formatClock('20:00', 'en-GB')).toBe('20:00');
  });

  it('gives nothing for something that is not a time', () => {
    expect(formatClock('', 'en-US')).toBe('');
    expect(formatClock('8pm', 'en-US')).toBe('');
    expect(formatClock('25:00', 'en-US')).toBe('');
  });
});

describe('zoneOffsetMinutes', () => {
  it('follows daylight saving', () => {
    expect(zoneOffsetMinutes('America/Chicago', JANUARY)).toBe(-360);
    expect(zoneOffsetMinutes('America/Chicago', JULY)).toBe(-300);
    expect(zoneOffsetMinutes('Asia/Kolkata', JULY)).toBe(330);
    expect(zoneOffsetMinutes('UTC', JULY)).toBe(0);
  });

  it('is null for a zone the webview does not know', () => {
    expect(zoneOffsetMinutes('Mars/Olympus', JULY)).toBeNull();
  });
});

describe('deviceClock', () => {
  it('converts the reminder time to the device’s clock', () => {
    expect(deviceClock('20:00', 'UTC', 'America/Chicago', JULY)).toBe('15:00');
    expect(deviceClock('20:00', 'UTC', 'Asia/Kolkata', JULY)).toBe('01:30');
    expect(deviceClock('01:00', 'UTC', 'America/Chicago', JANUARY)).toBe('19:00');
  });

  it('is null when both clocks read the same, or a zone is unknown', () => {
    expect(deviceClock('20:00', 'America/Chicago', 'America/Chicago', JULY)).toBeNull();
    expect(deviceClock('20:00', 'UTC', 'Etc/UTC', JULY)).toBeNull();
    expect(deviceClock('20:00', 'UTC', null, JULY)).toBeNull();
    expect(deviceClock('20:00', 'Mars/Olympus', 'America/Chicago', JULY)).toBeNull();
  });
});

describe('reminderSummary', () => {
  const base = {
    cadence: 'daily',
    time: '20:00',
    zone: 'America/Chicago',
    dm: true,
    channel: '#daily-sketch',
    deviceZone: 'America/Chicago',
    hasDays: true,
    now: JULY,
    locale: 'en-US',
  };

  it('says when, in which zone and how, and that it nudges once', () => {
    expect(plain(reminderSummary(base))).toBe(
      'Each day, leaf sends you a DM at 8:00 PM America/Chicago time (UTC-5) if that day has no post archived yet. It nudges once, then stays quiet until you archive the next day.',
    );
  });

  it('names the channel for a channel ping', () => {
    expect(reminderSummary({ ...base, dm: false })).toContain('pings you in #daily-sketch at');
    expect(reminderSummary({ ...base, dm: false, channel: null })).toContain(
      'pings you in the series channel at',
    );
  });

  it('describes each cadence’s own rule', () => {
    expect(reminderSummary({ ...base, cadence: 'weekdays' })).toMatch(/^Monday to Friday, leaf /);
    expect(reminderSummary({ ...base, cadence: 'weekly' })).toMatch(
      /^Each week, on the weekday of your last post, .* if nothing has been archived that week\./,
    );
  });

  it('gives the time on the device’s clock when the zones differ', () => {
    const text = plain(reminderSummary({ ...base, zone: 'UTC' }));
    expect(text).toContain('at 8:00 PM UTC time if');
    expect(text).toContain('That is 3:00 PM where you are.');
  });

  it('says nothing is sent before the first archived day, unless there is one', () => {
    const line = 'Nothing is sent before your first archived day.';
    expect(reminderSummary({ ...base, hasDays: false })).toContain(line);
    expect(reminderSummary({ ...base, hasDays: undefined })).toContain(line);
    expect(reminderSummary(base)).not.toContain(line);
  });

  it('is empty until a time is chosen', () => {
    expect(reminderSummary({ ...base, time: '' })).toBe('');
  });
});
