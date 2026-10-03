import { describe, expect, it } from 'vitest';

import type { SeriesSettings } from '../../types/api';
import {
  canEditStartDay,
  changedFields,
  reminderDefaults,
  settingsPatch,
  settingsProblems,
  settingsValues,
  unappliedFailure,
  type SettingsValues,
} from './settingsForm';

function saved(extra: Partial<SeriesSettings> = {}): SeriesSettings {
  return {
    id: 1,
    name: 'Daily Sketch',
    description: 'One drawing a day.',
    emoji: '✏️',
    cadence: 'daily',
    privacy: 'public',
    privacy_role_id: null,
    channel_id: 'c1',
    detection_mode: 'context_menu',
    state: 'active',
    reminder_enabled: false,
    reminder_time: null,
    reminder_timezone: null,
    reminder_dm: true,
    start_day: 1,
    reminder_error: undefined,
    reminder_error_at: undefined,
    ...extra,
  };
}

/** The form as loaded, with some edits on top. */
function edited(from: SeriesSettings, edits: Partial<SettingsValues>): SettingsValues {
  return { ...settingsValues(from), ...edits };
}

describe('settingsValues', () => {
  it('defaults nothing: a missing role, channel, time or zone stays empty', () => {
    expect(settingsValues(saved({ channel_id: null }))).toMatchObject({
      privacyRoleId: '',
      channelId: '',
      reminderTime: '',
      reminderTz: '',
      startDay: '1',
    });
  });

  it('pads a time stored unpadded so a time input can show it', () => {
    expect(settingsValues(saved({ reminder_time: '9:05' })).reminderTime).toBe('09:05');
  });
});

describe('changedFields', () => {
  it('finds nothing changed in a freshly loaded form, whatever is stored', () => {
    const odd = [
      saved(),
      saved({ channel_id: null }),
      saved({ channel_id: 'no-longer-watched' }),
      saved({ privacy: 'role_gated', privacy_role_id: null }),
      saved({ description: ' padded ', emoji: '' }),
      saved({ cadence: 'freeform', reminder_enabled: true, reminder_time: '20:00' }),
      saved({
        reminder_enabled: true,
        reminder_time: '9:05',
        reminder_timezone: 'America/Chicago',
      }),
      saved({ start_day: undefined }),
      saved({ start_day: 5_000_000 }),
    ];
    for (const settings of odd) {
      expect(changedFields(settings, settingsValues(settings))).toEqual([]);
    }
  });

  it('ignores whitespace around text the server trims', () => {
    const s = saved();
    expect(changedFields(s, edited(s, { name: ' Daily Sketch ', startDay: '01' }))).toEqual([]);
  });

  it('lists exactly what was edited', () => {
    const s = saved();
    expect(changedFields(s, edited(s, { description: 'New', emoji: '🍃' }))).toEqual([
      'description',
      'emoji',
    ]);
    expect(changedFields(s, edited(s, { name: 'Renamed', startDay: '0' }))).toEqual([
      'name',
      'startDay',
    ]);
  });

  it('sends the role along whenever the series becomes role-only', () => {
    const s = saved({ privacy_role_id: 'r1' });
    expect(changedFields(s, edited(s, { privacy: 'role_gated' }))).toEqual(['privacy', 'role']);
    // No role chosen yet: only the privacy is a change (and a problem).
    const none = saved();
    expect(changedFields(none, edited(none, { privacy: 'role_gated' }))).toEqual(['privacy']);
    // Leaving role-only does not touch the stored role.
    const gated = saved({ privacy: 'role_gated', privacy_role_id: 'r1' });
    expect(changedFields(gated, edited(gated, { privacy: 'public' }))).toEqual(['privacy']);
    expect(changedFields(gated, edited(gated, { privacyRoleId: 'r2' }))).toEqual(['role']);
  });

  it('leaves the first day alone when what it is now is not known', () => {
    const unknown = saved({ start_day: undefined });
    expect(canEditStartDay(unknown)).toBe(false);
    expect(settingsValues(unknown).startDay).toBe('');
    // The name does not depend on it.
    expect(changedFields(unknown, edited(unknown, { name: 'Renamed', startDay: '7' }))).toEqual([
      'name',
    ]);
  });

  it('counts the reminder’s details only while it is on', () => {
    const s = saved();
    expect(changedFields(s, edited(s, { reminderTime: '20:00', reminderTz: 'UTC' }))).toEqual([]);
    expect(changedFields(s, edited(s, { reminderEnabled: true, reminderTime: '20:00' }))).toEqual([
      'reminderEnabled',
      'reminderTime',
    ]);
    // Switched back on with the time, zone and delivery it already had.
    const had = saved({ reminder_time: '21:00', reminder_timezone: 'UTC', reminder_dm: false });
    expect(changedFields(had, edited(had, { reminderEnabled: true }))).toEqual(['reminderEnabled']);
  });

  it('treats a freeform series’ reminder as off', () => {
    const s = saved({ reminder_enabled: true, reminder_time: '20:00' });
    expect(changedFields(s, edited(s, { cadence: 'freeform' }))).toEqual([
      'cadence',
      'reminderEnabled',
    ]);
  });

  it('compares timezones without regard to case', () => {
    const s = saved({ reminder_enabled: true, reminder_time: '20:00', reminder_timezone: 'UTC' });
    expect(changedFields(s, edited(s, { reminderTz: 'utc' }))).toEqual([]);
    expect(changedFields(s, edited(s, { reminderTz: '' }))).toEqual(['reminderTz']);
  });
});

describe('settingsPatch', () => {
  it('is empty for an untouched form, so a stale channel is never re-sent', () => {
    const s = saved({ channel_id: 'no-longer-watched' });
    expect(settingsPatch(s, settingsValues(s))).toEqual({});
  });

  it('holds only the changed fields, trimmed', () => {
    const s = saved({ channel_id: 'no-longer-watched' });
    expect(settingsPatch(s, edited(s, { description: ' Fresh words ' }))).toEqual({
      description: 'Fresh words',
    });
    expect(
      settingsPatch(s, edited(s, { name: ' Renamed ', emoji: ' 🍃 ', startDay: '200' })),
    ).toEqual({ name: 'Renamed', emoji: '🍃', start_day: 200 });
  });

  it('never sends detection_mode', () => {
    const s = saved({ detection_mode: 'passive' });
    const patch = settingsPatch(
      s,
      edited(s, { description: 'x', cadence: 'weekly', channelId: 'c2' }),
    );
    expect(patch).toEqual({ description: 'x', cadence: 'weekly', channel_id: 'c2' });
    expect('detection_mode' in patch).toBe(false);
  });

  it('sends what switching the reminder on changed, and no empty timezone', () => {
    const s = saved();
    expect(
      settingsPatch(
        s,
        edited(s, { reminderEnabled: true, reminderTime: '20:00', reminderTz: 'America/Chicago' }),
      ),
    ).toEqual({
      reminder_enabled: true,
      reminder_time: '20:00',
      reminder_timezone: 'America/Chicago',
    });
    // The server's timezone is already what an unset override means.
    expect(settingsPatch(s, edited(s, { reminderEnabled: true, reminderTime: '20:00' }))).toEqual({
      reminder_enabled: true,
      reminder_time: '20:00',
    });
    expect(
      settingsPatch(
        s,
        edited(s, { reminderEnabled: true, reminderTime: '20:00', reminderDm: false }),
      ),
    ).toEqual({ reminder_enabled: true, reminder_time: '20:00', reminder_dm: false });
  });

  it('sends only the switch when the reminder is turned off', () => {
    const s = saved({ reminder_enabled: true, reminder_time: '20:00' });
    expect(settingsPatch(s, edited(s, { reminderEnabled: false, reminderTime: '21:00' }))).toEqual({
      reminder_enabled: false,
    });
  });

  it('clears the timezone override with an empty string', () => {
    const s = saved({ reminder_enabled: true, reminder_time: '20:00', reminder_timezone: 'UTC' });
    expect(settingsPatch(s, edited(s, { reminderTz: '' }))).toEqual({ reminder_timezone: '' });
  });

  it('turns reminders off when the cadence becomes freeform', () => {
    const s = saved({ reminder_enabled: true, reminder_time: '20:00' });
    expect(settingsPatch(s, edited(s, { cadence: 'freeform' }))).toEqual({
      cadence: 'freeform',
      reminder_enabled: false,
    });
  });
});

describe('settingsProblems', () => {
  it('checks only what is being sent', () => {
    // A name and emoji stored before the rules existed do not block other edits.
    const s = saved({ name: 'x', emoji: 'abc' });
    expect(settingsProblems(s, edited(s, { description: 'Fine' }))).toEqual({});
  });

  it('reports each changed field the server would refuse', () => {
    const s = saved();
    const problems = settingsProblems(
      s,
      edited(s, {
        name: '@everyone',
        description: 'x'.repeat(201),
        emoji: 'two words',
        startDay: 'abc',
      }),
    );
    expect(Object.keys(problems).sort()).toEqual(['description', 'emoji', 'name', 'startDay']);
    expect(problems.emoji).toMatch(/single standard emoji/);
  });

  it('needs a role for a role-only series, even one stored without it', () => {
    const s = saved({ privacy: 'role_gated', privacy_role_id: null });
    expect(settingsProblems(s, edited(s, { description: 'x' })).role).toBe(
      'Choose the role that can view this series.',
    );
  });

  it('needs a time once reminders are on', () => {
    const s = saved();
    expect(settingsProblems(s, edited(s, { reminderEnabled: true })).reminderTime).toBe(
      'Choose a reminder time before turning reminders on.',
    );
    expect(
      settingsProblems(s, edited(s, { reminderEnabled: true, reminderTime: '8pm' })).reminderTime,
    ).toMatch(/HH:MM/);
    expect(
      settingsProblems(s, edited(s, { reminderEnabled: true, reminderTime: '20:00' })),
    ).toEqual({});
    // Freeform: the reminder is off whatever the box says, so no time is needed.
    expect(settingsProblems(s, edited(s, { reminderEnabled: true, cadence: 'freeform' }))).toEqual(
      {},
    );
  });

  it('refuses a typed timezone the webview does not know', () => {
    const s = saved();
    const on = { reminderEnabled: true, reminderTime: '20:00' };
    expect(
      settingsProblems(s, edited(s, { ...on, reminderTz: 'Mars/Olympus' })).reminderTz,
    ).toMatch(/doesn’t know that timezone/);
    expect(settingsProblems(s, edited(s, { ...on, reminderTz: 'europe/berlin' }))).toEqual({});
  });
});

describe('unappliedFailure', () => {
  const before = saved();

  it('is null when the answer shows everything that was sent', () => {
    expect(unappliedFailure(before, { description: 'New' }, saved({ description: 'New' }))).toBe(
      null,
    );
    expect(
      unappliedFailure(
        before,
        { name: 'Renamed', start_day: 200 },
        saved({ name: 'Renamed', start_day: 200 }),
      ),
    ).toBe(null);
    // The server may store a name its own way; any change counts as applied.
    expect(unappliedFailure(before, { name: 'Renamed' }, saved({ name: 'renamed' }))).toBe(null);
  });

  it('says so, under the name, when a rename came back unchanged', () => {
    const failure = unappliedFailure(before, { name: 'Renamed' }, saved());
    expect(failure?.code).toBe('name_not_saved');
    expect(failure?.message).toMatch(/^The name wasn’t changed\./);
    expect(failure?.message).toMatch(/update leaf\.$/);
  });

  it('says so, under the first day number, when that came back unchanged', () => {
    const failure = unappliedFailure(before, { start_day: 200 }, saved());
    expect(failure?.code).toBe('start_day_not_saved');
    expect(failure?.message).toMatch(/^The first day number wasn’t changed\./);
  });

  it('names both, and says the rest was saved', () => {
    const failure = unappliedFailure(
      before,
      { name: 'Renamed', start_day: 200, emoji: '🍃' },
      saved({ emoji: '🍃' }),
    );
    expect(failure?.code).toBe('name_not_saved');
    expect(failure?.message).toMatch(/^The name and first day number weren’t changed\./);
    expect(failure?.message).toMatch(/Your other changes were saved\.$/);
  });

  it('does not doubt a first day number it has nothing to compare with', () => {
    const unknown = saved({ start_day: undefined });
    expect(unappliedFailure(unknown, { emoji: '🍃' }, saved({ emoji: '🍃' }))).toBe(null);
  });
});

describe('reminderDefaults', () => {
  it('fills in early evening in the device’s zone the first time', () => {
    expect(reminderDefaults({ reminderTime: '', reminderTz: '' }, 'Europe/Berlin')).toEqual({
      reminderTime: '20:00',
      reminderTz: 'Europe/Berlin',
    });
  });

  it('falls back to the server’s zone when the device will not say', () => {
    expect(reminderDefaults({ reminderTime: '', reminderTz: '' }, null)).toEqual({
      reminderTime: '20:00',
      reminderTz: '',
    });
  });

  it('keeps a time and zone that were already chosen', () => {
    expect(reminderDefaults({ reminderTime: '07:30', reminderTz: '' }, 'Europe/Berlin')).toEqual({
      reminderTime: '07:30',
      reminderTz: '',
    });
    expect(reminderDefaults({ reminderTime: '', reminderTz: 'UTC' }, 'Europe/Berlin')).toEqual({
      reminderTime: '20:00',
      reminderTz: 'UTC',
    });
  });
});
