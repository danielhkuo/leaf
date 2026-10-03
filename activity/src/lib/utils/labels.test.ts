import { describe, expect, it } from 'vitest';

import { reminderErrorMessage, seriesErrorMessage, violationMessage } from './labels';

// 1 Oct 2026, 12:00 local time.
const NOW = new Date(2026, 9, 1, 12, 0, 0).getTime();
const unix = (d: Date): number => Math.floor(d.getTime() / 1000);

describe('seriesErrorMessage', () => {
  it('falls back for a missing or unknown code', () => {
    expect(seriesErrorMessage(undefined, 'Fallback.')).toBe('Fallback.');
    expect(seriesErrorMessage('brand_new_code', 'Fallback.')).toBe('Fallback.');
    // Not a key leaf defines, even though every object has one.
    expect(seriesErrorMessage('constructor', 'Fallback.')).toBe('Fallback.');
  });

  it('says why a taken name may not be visible', () => {
    const text = seriesErrorMessage('name_taken', '');
    expect(text).toContain('private or revoked');
    expect(text).toContain('Choose a different name');
  });

  it('tells the creator of a revoked series who can restore it', () => {
    expect(seriesErrorMessage('revoked', '')).toContain('Ask an admin to restore it');
  });

  it('gives every sentence a capital and a full stop', () => {
    const codes = [
      'max_series',
      'account_too_new',
      'membership_too_new',
      'missing_creator_role',
      'guild_not_setup',
      'invalid_channel',
      'unknown_channel',
      'unknown_role',
      'invalid_name',
      'invalid_description',
      'invalid_emoji',
      'invalid_start_day',
      'missing_privacy_role',
      'role_required',
      'invalid_reminder_time',
      'invalid_timezone',
      'reminder_time_required',
      'reminder_on_freeform',
      'name_taken',
      'revoked',
      'discord_unavailable',
      'code_rejected',
    ];
    for (const code of codes) {
      const text = seriesErrorMessage(code, 'FALLBACK');
      expect(text, code).not.toBe('FALLBACK');
      expect(text, code).toMatch(/^[A-Zl].*\.$/);
    }
  });
});

describe('violationMessage', () => {
  it('names the count and the limit', () => {
    expect(
      violationMessage({ code: 'max_series', message: '', params: { limit: 3, current: 3 } }),
    ).toBe(
      'You have 3 of 3 series here, which is this server’s limit. Ask a server admin if you need another.',
    );
    expect(violationMessage({ code: 'max_series', message: '', params: { limit: 3 } })).toContain(
      'limit of 3 series',
    );
    expect(violationMessage({ code: 'max_series', message: '' })).toContain('Ask a server admin');
  });

  it('names the role to ask for', () => {
    expect(
      violationMessage({
        code: 'missing_creator_role',
        message: '',
        params: { role_name: 'Artists' },
      }),
    ).toBe('Starting a series here needs the @Artists role. Ask a server admin for it.');
    expect(violationMessage({ code: 'missing_creator_role', message: '' })).toContain(
      'a role you don’t have',
    );
  });

  it('gives the date an age rule lifts', () => {
    const text = violationMessage(
      {
        code: 'account_too_new',
        message: '',
        params: { days: 30, eligible_at: unix(new Date(2026, 9, 14, 9, 0, 0)) },
      },
      NOW,
    );
    expect(text).toMatch(/You can start one on .*14/);
    expect(text).toContain('Oct');
    expect(text).not.toContain('2026');
  });

  it('adds the year when the date is in another year', () => {
    const text = violationMessage(
      {
        code: 'membership_too_new',
        message: '',
        params: { eligible_at: unix(new Date(2027, 0, 5, 9, 0, 0)) },
      },
      NOW,
    );
    expect(text).toContain('2027');
  });

  it('says "later today" for a time still ahead today', () => {
    const text = violationMessage(
      {
        code: 'membership_too_new',
        message: '',
        params: { eligible_at: unix(new Date(2026, 9, 1, 18, 0, 0)) },
      },
      NOW,
    );
    expect(text).toContain('You can start one later today.');
  });

  it('falls back to the day count once the date has passed or is unknown', () => {
    const past = violationMessage(
      {
        code: 'account_too_new',
        message: '',
        params: { days: 1, eligible_at: unix(new Date(2026, 8, 1)) },
      },
      NOW,
    );
    expect(past).toBe(
      'Your Discord account needs to be at least 1 day old to start a series here.',
    );
    expect(
      violationMessage({ code: 'membership_too_new', message: '', params: { days: 7 } }, NOW),
    ).toContain('at least 7 days');
  });

  it('names /setup and who runs it for a server that is not set up', () => {
    expect(violationMessage({ code: 'guild_not_setup', message: 'not set up' })).toBe(
      'leaf isn’t set up in this server yet. A server admin needs to run /setup in chat first.',
    );
  });

  it('uses the server’s sentence for a code it does not know', () => {
    expect(violationMessage({ code: 'new_rule', message: 'A new rule applies.' })).toBe(
      'A new rule applies.',
    );
    expect(violationMessage({ code: 'new_rule', message: '' })).toContain('Ask a server admin');
  });
});

describe('reminderErrorMessage', () => {
  it('has specific advice for each recorded reason', () => {
    expect(reminderErrorMessage('dm_closed')).toContain('Privacy Settings');
    expect(reminderErrorMessage('channel_missing')).toContain('Pick a channel');
    expect(reminderErrorMessage('no_permission')).toContain('Ask a server admin');
  });

  it('falls back to a general line', () => {
    expect(reminderErrorMessage('something_else')).toContain('couldn’t deliver the last reminder');
  });
});
