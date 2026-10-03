// The sentence under the reminder controls: when leaf will nudge, in which
// timezone, how, and how often. The rules it describes are leaf-core's
// `reminder_due`: at the chosen time on a day (or week) with no post yet,
// once per missing day number, and never before the first archived day.

import { timezoneOffsetLabel } from '../../utils/timezones';

const HH_MM = /^(\d{1,2}):(\d{2})$/;

function minutesOfDay(time: string): number | null {
  const parts = HH_MM.exec(time);
  if (!parts) return null;
  const minutes = Number(parts[1]) * 60 + Number(parts[2]);
  return minutes < 24 * 60 ? minutes : null;
}

/** `20:00` as the device's locale writes a time ("8:00 PM", "20:00"). `''` if it is not a time. */
export function formatClock(time: string, locale?: string): string {
  const minutes = minutesOfDay(time);
  if (minutes === null) return '';
  // A local date formatted in the local zone: the clock reads back unchanged.
  const at = new Date(2000, 0, 1, Math.floor(minutes / 60), minutes % 60);
  return new Intl.DateTimeFormat(locale, { hour: 'numeric', minute: '2-digit' }).format(at);
}

/**
 * How far `zone`'s clock is ahead of UTC at `at`, in minutes (negative when
 * behind). `null` when the webview does not know the zone.
 */
export function zoneOffsetMinutes(zone: string, at: Date): number | null {
  try {
    const parts = new Intl.DateTimeFormat('en-US', {
      timeZone: zone,
      hourCycle: 'h23',
      year: 'numeric',
      month: 'numeric',
      day: 'numeric',
      hour: 'numeric',
      minute: 'numeric',
      second: 'numeric',
    }).formatToParts(at);
    const part = (type: Intl.DateTimeFormatPartTypes): number =>
      Number(parts.find((p) => p.type === type)?.value);
    const wall = Date.UTC(
      part('year'),
      part('month') - 1,
      part('day'),
      part('hour'),
      part('minute'),
      part('second'),
    );
    const minutes = Math.round((wall - at.getTime()) / 60_000);
    return Number.isFinite(minutes) ? minutes : null;
  } catch {
    return null;
  }
}

/**
 * The same moment on the device's clock, when a reminder's timezone is not
 * the device's: `20:00` in UTC is `15:00` in Chicago. `null` when the two
 * clocks agree or either zone is unknown.
 */
export function deviceClock(
  time: string,
  zone: string,
  deviceZone: string | null,
  at: Date = new Date(),
): string | null {
  const minutes = minutesOfDay(time);
  if (minutes === null || !deviceZone || deviceZone === zone) return null;
  const there = zoneOffsetMinutes(zone, at);
  const here = zoneOffsetMinutes(deviceZone, at);
  if (there === null || here === null || there === here) return null;
  const local = (((minutes + here - there) % 1440) + 1440) % 1440;
  const hh = String(Math.floor(local / 60)).padStart(2, '0');
  const mm = String(local % 60).padStart(2, '0');
  return `${hh}:${mm}`;
}

export interface ReminderPlan {
  cadence: string;
  /** `HH:MM`. */
  time: string;
  /** The zone the time is read in: the series' own, else the server's. */
  zone: string;
  dm: boolean;
  /** The series channel as shown (`#daily-sketch`), when known. */
  channel: string | null;
  deviceZone: string | null;
  /** Whether a day is archived yet; `undefined` when not known. */
  hasDays?: boolean | undefined;
  now?: Date;
  locale?: string;
}

/** When and how leaf will nudge, as plain sentences. `''` until a time is chosen. */
export function reminderSummary(plan: ReminderPlan): string {
  const { cadence, time, zone, dm, channel, deviceZone, hasDays, now = new Date(), locale } = plan;
  const clock = formatClock(time, locale);
  if (clock === '') return '';

  const offset = timezoneOffsetLabel(zone, now);
  const when = `${clock} ${zone} time${offset && offset !== zone ? ` (${offset})` : ''}`;
  const how = dm ? 'sends you a DM' : `pings you in ${channel ?? 'the series channel'}`;
  let rule: string;
  if (cadence === 'weekly') {
    rule = `Each week, on the weekday of your last post, leaf ${how} at ${when} if nothing has been archived that week.`;
  } else if (cadence === 'weekdays') {
    rule = `Monday to Friday, leaf ${how} at ${when} if that day has no post archived yet.`;
  } else {
    rule = `Each day, leaf ${how} at ${when} if that day has no post archived yet.`;
  }

  const sentences = [rule];
  const here = deviceClock(time, zone, deviceZone, now);
  if (here) sentences.push(`That is ${formatClock(here, locale)} where you are.`);
  sentences.push('It nudges once, then stays quiet until you archive the next day.');
  if (hasDays !== true) sentences.push('Nothing is sent before your first archived day.');
  return sentences.join(' ');
}
