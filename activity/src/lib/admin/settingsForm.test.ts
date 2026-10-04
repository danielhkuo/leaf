import { describe, expect, it } from 'vitest';

import type { AdminSettings } from './schemas';
import {
  changedFields,
  draftOf,
  fieldForCode,
  parseWhole,
  readDraft,
  savedText,
  settingsPatch,
  settingsProblems,
  settingsValues,
  withDraft,
  type SettingsValues,
} from './settingsForm';

const ROLE = '111111111111111111';
const CHANNEL = '222222222222222222';

function saved(extra: Partial<AdminSettings> = {}): AdminSettings {
  return {
    timezone: 'America/Chicago',
    creator_role_id: ROLE,
    log_channel_id: null,
    max_series_per_user: 3,
    min_account_age_days: 30,
    min_membership_age_days: 7,
    sprout_enabled: true,
    sprout_threshold: 3,
    ...extra,
  };
}

function form(extra: Partial<SettingsValues> = {}, base: AdminSettings = saved()): SettingsValues {
  return { ...settingsValues(base), ...extra };
}

describe('settingsValues', () => {
  it('shows a missing role or channel as blank and numbers as text', () => {
    expect(settingsValues(saved())).toEqual({
      timezone: 'America/Chicago',
      creatorRoleId: ROLE,
      logChannelId: '',
      maxSeries: '3',
      minAccountAge: '30',
      minMembershipAge: '7',
      sproutEnabled: true,
      sproutThreshold: '3',
    });
  });
});

describe('parseWhole', () => {
  it('reads digits, full-width digits and surrounding space', () => {
    expect(parseWhole(' 12 ')).toBe(12);
    expect(parseWhole('１２')).toBe(12);
    expect(parseWhole('007')).toBe(7);
  });

  it('refuses anything that is not a whole number', () => {
    for (const raw of ['', ' ', '-1', '1.5', '1e3', 'three', '1 2']) {
      expect(parseWhole(raw)).toBeNull();
    }
  });
});

describe('changedFields', () => {
  it('finds nothing to save in an untouched form', () => {
    expect(changedFields(saved(), form())).toEqual([]);
  });

  it('does not count spelling that the server would store the same way', () => {
    const same = form({
      timezone: ' america/chicago ',
      maxSeries: '03',
      creatorRoleId: ` ${ROLE} `,
    });
    expect(changedFields(saved(), same)).toEqual([]);
  });

  it('lists each changed field in form order', () => {
    const edited = form({ logChannelId: CHANNEL, timezone: 'UTC', sproutThreshold: '5' });
    expect(changedFields(saved(), edited)).toEqual(['timezone', 'logChannelId', 'sproutThreshold']);
  });

  it('counts an emptied number as a change, so it is checked rather than skipped', () => {
    expect(changedFields(saved(), form({ maxSeries: '' }))).toEqual(['maxSeries']);
  });

  it('ignores the threshold while the sprout stage is switched off', () => {
    const off = form({ sproutEnabled: false, sproutThreshold: '9' });
    expect(changedFields(saved(), off)).toEqual(['sproutEnabled']);
  });
});

describe('settingsProblems', () => {
  it('passes a valid edit', () => {
    const edited = form({ timezone: 'Europe/Berlin', maxSeries: '5', logChannelId: CHANNEL });
    expect(settingsProblems(saved(), edited)).toEqual({});
  });

  it('refuses a timezone the browser does not know, and a blank one', () => {
    expect(settingsProblems(saved(), form({ timezone: 'CST6' })).timezone).toMatch(/doesn’t know/);
    expect(settingsProblems(saved(), form({ timezone: ' ' })).timezone).toBe('Choose a timezone.');
  });

  it('never blocks a save on a stored value that was not touched', () => {
    const legacy = saved({
      timezone: 'Chicago time',
      creator_role_id: 'r1',
      max_series_per_user: 0,
    });
    expect(settingsProblems(legacy, form({ minAccountAge: '10' }, legacy))).toEqual({});
  });

  it('requires at least one series per member and whole numbers of days', () => {
    expect(settingsProblems(saved(), form({ maxSeries: '0' })).maxSeries).toBe(
      'Enter a whole number, 1 or more.',
    );
    expect(settingsProblems(saved(), form({ maxSeries: '' })).maxSeries).toBe(
      'Enter a whole number, 1 or more.',
    );
    expect(settingsProblems(saved(), form({ minAccountAge: '2.5' })).minAccountAge).toBe(
      'Enter a whole number of days, 0 or more.',
    );
    expect(settingsProblems(saved(), form({ minMembershipAge: '0' }))).toEqual({});
    expect(settingsProblems(saved(), form({ sproutThreshold: '0' })).sproutThreshold).toBe(
      'Enter a whole number of days, 1 or more.',
    );
  });

  it('caps the numbers', () => {
    expect(settingsProblems(saved(), form({ minAccountAge: '1000000' })).minAccountAge).toMatch(
      /999999 or less/,
    );
  });

  it('wants a pasted id to look like one, but trusts an id the picker listed', () => {
    const typo = form({ creatorRoleId: '12345', logChannelId: 'general' });
    const problems = settingsProblems(saved(), typo);
    expect(problems.creatorRoleId).toMatch(/role ID is a number/);
    expect(problems.logChannelId).toMatch(/channel ID is a number/);

    const listed = settingsProblems(saved(), form({ creatorRoleId: 'r2', logChannelId: 'c9' }), {
      roles: ['r2'],
      channels: ['c9'],
    });
    expect(listed).toEqual({});
  });

  it('lets the role and the channel be cleared', () => {
    expect(settingsProblems(saved(), form({ creatorRoleId: '' }))).toEqual({});
  });
});

describe('settingsPatch', () => {
  it('sends only what changed, with numbers as numbers', () => {
    const edited = form({ maxSeries: ' 5 ', sproutEnabled: false, timezone: 'Europe/Berlin' });
    expect(settingsPatch(saved(), edited)).toEqual({
      timezone: 'Europe/Berlin',
      max_series_per_user: 5,
      sprout_enabled: false,
    });
  });

  it('clears the role and the channel with an empty string', () => {
    const base = saved({ log_channel_id: CHANNEL });
    const cleared = form({ creatorRoleId: '', logChannelId: '' }, base);
    expect(settingsPatch(base, cleared)).toEqual({ creator_role_id: '', log_channel_id: '' });
  });

  it('is empty for an untouched form, so a stale panel overwrites nothing', () => {
    expect(settingsPatch(saved(), form())).toEqual({});
  });
});

describe('drafts', () => {
  it('keeps the changed fields as typed, valid or not', () => {
    expect(draftOf(saved(), form({ maxSeries: '', timezone: 'UTC' }))).toEqual({
      timezone: 'UTC',
      maxSeries: '',
    });
    expect(draftOf(saved(), form())).toBeNull();
  });

  it('lays a kept draft over freshly loaded settings', () => {
    // /setup changed the log channel while the admin was away: that is kept.
    const fresh = saved({ log_channel_id: CHANNEL });
    const values = withDraft(fresh, { maxSeries: '9' });
    expect(values.maxSeries).toBe('9');
    expect(values.logChannelId).toBe(CHANNEL);
    expect(withDraft(fresh, null)).toEqual(settingsValues(fresh));
  });

  it('accepts a stored draft only in the shape it wrote', () => {
    expect(readDraft({ timezone: 'UTC', sproutEnabled: false })).toEqual({
      timezone: 'UTC',
      sproutEnabled: false,
    });
    expect(readDraft({ timezone: 5 })).toBeNull();
    expect(readDraft({})).toBeNull();
    expect(readDraft(null)).toBeNull();
    expect(readDraft('nope')).toBeNull();
    // Keys from another build are dropped, not kept.
    expect(readDraft({ maxSeries: '4', somethingElse: true })).toEqual({ maxSeries: '4' });
  });
});

describe('fieldForCode', () => {
  it('places the codes that name a field, and leaves the rest to the form', () => {
    expect(fieldForCode('invalid_timezone')).toBe('timezone');
    expect(fieldForCode('unknown_role')).toBe('creatorRoleId');
    expect(fieldForCode('unknown_channel')).toBe('logChannelId');
    expect(fieldForCode('invalid_limit')).toBeNull();
    expect(fieldForCode('toString')).toBeNull();
    expect(fieldForCode(undefined)).toBeNull();
  });
});

describe('savedText', () => {
  it('adds what the save published', () => {
    expect(savedText(saved())).toBe('Saved');
    expect(savedText(saved({ sprouts_published: 0 }))).toBe('Saved');
    expect(savedText(saved({ sprouts_published: 1 }))).toBe('Saved. 1 sprout was published.');
    expect(savedText(saved({ sprouts_published: 4 }))).toBe('Saved. 4 sprouts were published.');
  });
});
