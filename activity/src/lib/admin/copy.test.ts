import { describe, expect, it } from 'vitest';

import { AdminApiError } from './client';
import {
  adminErrorMessage,
  adminPrivacyLabel,
  canRetry,
  creatorLabel,
  isGone,
  NO_GUILDS,
  outcomeUnknown,
  privacyQuestion,
  sessionExpired,
  signInProblem,
  stateLabel,
  unconfirmedMessage,
} from './copy';

const refused = (status: number, code?: string, detail?: string): AdminApiError =>
  new AdminApiError(status, `PATCH /x → ${status}`, code, detail === undefined ? {} : { detail });

describe('adminErrorMessage', () => {
  it('never shows the developer label', () => {
    for (const status of [400, 401, 403, 404, 422, 500, 503]) {
      expect(adminErrorMessage(refused(status))).not.toContain('→');
    }
  });

  it('has its own sentence for the codes the admin API answers with', () => {
    expect(adminErrorMessage(refused(422, 'invalid_timezone'))).toMatch(/timezone/);
    expect(adminErrorMessage(refused(422, 'unknown_role'))).toMatch(/role/);
    expect(adminErrorMessage(refused(422, 'unknown_channel'))).toMatch(/channel/);
    expect(adminErrorMessage(refused(422, 'role_required'))).toMatch(/Choose the role/);
    expect(adminErrorMessage(refused(503, 'discord_unavailable'))).toMatch(/Discord/);
  });

  it('uses the server’s sentence for a refused value it has no copy for', () => {
    expect(adminErrorMessage(refused(422, 'invalid_limit', 'Series per member must be 1.'))).toBe(
      'Series per member must be 1.',
    );
    expect(adminErrorMessage(refused(422, 'invalid_limit'))).toMatch(/Check the values/);
  });

  it('ignores a code that only looks like one of its keys', () => {
    expect(adminErrorMessage(refused(422, 'constructor'))).toMatch(/Check the values/);
  });

  it('tells a lost connection from a slow server and from a server fault', () => {
    const network = new AdminApiError(0, 'GET /x', undefined, { kind: 'network' });
    const timeout = new AdminApiError(0, 'GET /x', undefined, { kind: 'timeout' });
    expect(adminErrorMessage(network)).toMatch(/connection/);
    expect(adminErrorMessage(timeout)).toMatch(/too long/);
    expect(adminErrorMessage(refused(500))).toMatch(/leaf had a problem/);
  });

  it('words a 404 for what was being changed', () => {
    expect(adminErrorMessage(refused(404))).toMatch(/server is no longer available/);
    expect(adminErrorMessage(refused(404), 'series')).toMatch(/series any more/);
  });

  it('says to sign in again for an expired session', () => {
    expect(adminErrorMessage(refused(401))).toMatch(/Sign in again/);
  });

  it('has a line for anything that is not an API failure', () => {
    expect(adminErrorMessage(new TypeError('boom'))).toBe('Something went wrong. Try again.');
  });
});

describe('a change that got no answer', () => {
  const lost = (kind: 'network' | 'timeout' | 'bad_response'): AdminApiError =>
    new AdminApiError(0, 'PATCH /x', undefined, { kind });

  it('is only known to have failed when leaf refused it', () => {
    for (const status of [400, 403, 404, 422]) {
      expect(outcomeUnknown(refused(status))).toBe(false);
    }
    expect(outcomeUnknown(refused(422, 'role_required'))).toBe(false);
  });

  it('may have gone through when no answer came, or none that settles it', () => {
    expect(outcomeUnknown(lost('network'))).toBe(true);
    expect(outcomeUnknown(lost('timeout'))).toBe(true);
    expect(outcomeUnknown(lost('bad_response'))).toBe(true);
    expect(outcomeUnknown(refused(500))).toBe(true);
    expect(outcomeUnknown(refused(504))).toBe(true);
    expect(outcomeUnknown(new TypeError('boom'))).toBe(true);
  });

  it('says the page can’t tell, never that the change failed', () => {
    const causes = [
      lost('network'),
      lost('timeout'),
      lost('bad_response'),
      refused(502),
      new TypeError('boom'),
    ];
    for (const cause of causes) {
      const message = unconfirmedMessage(cause);
      expect(message).toMatch(/can’t tell whether the change went through/);
      expect(message).toMatch(/[Cc]heck again/);
      expect(message).not.toMatch(/wasn’t|→/);
    }
    expect(unconfirmedMessage(lost('network'))).toMatch(/connection/);
    expect(unconfirmedMessage(lost('timeout'))).toMatch(/didn’t answer in time/);
  });
});

describe('canRetry and isGone', () => {
  it('offers a retry for passing failures only', () => {
    expect(canRetry(refused(500))).toBe(true);
    expect(canRetry(new AdminApiError(0, 'x', undefined, { kind: 'network' }))).toBe(true);
    expect(canRetry(refused(422))).toBe(false);
    expect(canRetry(new Error('unknown'))).toBe(true);
  });

  it('knows when a sign-in no longer covers the server', () => {
    expect(isGone(refused(404))).toBe(true);
    expect(isGone(refused(403))).toBe(true);
    expect(isGone(refused(500))).toBe(false);
  });
});

describe('sign-in copy', () => {
  it('has a title and a next step for every callback code', () => {
    for (const code of ['denied', 'expired', 'exchange_failed', 'discord_unavailable']) {
      const problem = signInProblem(code, 'https://leaf.example');
      expect(problem.title).not.toBe('');
      expect(problem.message).not.toBe('');
      expect(problem.title).not.toBe(signInProblem('something_new', '').title);
    }
  });

  it('names the redirect to check when Discord rejects the sign-in', () => {
    expect(signInProblem('exchange_failed', 'https://leaf.example').message).toContain(
      'https://leaf.example/admin/callback',
    );
  });

  it('explains what a server needs before it can be managed', () => {
    expect(signInProblem('no_guilds', '')).toBe(NO_GUILDS);
    expect(NO_GUILDS.message).toMatch(/Manage Server/);
    expect(NO_GUILDS.message).toMatch(/leaf’s bot has to be in that server/);
    // The bot makes a server's settings row when it first sees the server,
    // so /setup is not what is missing.
    expect(NO_GUILDS.message).not.toMatch(/\/setup/);
  });

  it('falls back to a general line for a code it does not know', () => {
    expect(signInProblem('teapot', '').title).toBe('Sign-in didn’t finish');
  });

  it('says whether unsaved settings were kept when the session ran out', () => {
    expect(sessionExpired(true).message).toMatch(/unsaved settings are kept/);
    expect(sessionExpired(false).message).not.toMatch(/unsaved/);
  });
});

describe('series labels', () => {
  it('uses the gallery’s privacy words, except that the creator is not "me"', () => {
    expect(adminPrivacyLabel('public')).toBe('Everyone in the server');
    expect(adminPrivacyLabel('role_gated')).toBe('Only members with a role');
    expect(adminPrivacyLabel('creator_only')).toBe('Only its creator');
  });

  it('names the creator, or falls back to the id', () => {
    expect(creatorLabel({ creator_id: '42', creator_name: 'Mika' })).toBe('by Mika');
    expect(creatorLabel({ creator_id: '42', creator_name: undefined })).toBe('by member 42');
  });

  it('shows a sprout’s progress when the server reports it', () => {
    expect(stateLabel({ state: 'sprout', archived_days: 2 }, 3)).toBe(
      '🌱 Sprout · 2 of 3 days archived',
    );
    expect(stateLabel({ state: 'sprout', archived_days: 0 }, 1)).toBe(
      '🌱 Sprout · 0 of 1 day archived',
    );
    expect(stateLabel({ state: 'sprout', archived_days: undefined }, 3)).toBe('🌱 Sprout');
    expect(stateLabel({ state: 'active', archived_days: 9 }, 3)).toBe('Active');
    expect(stateLabel({ state: 'revoked', archived_days: undefined }, 3)).toBe('Revoked');
  });

  it('asks what a privacy change will do before it is saved', () => {
    expect(privacyQuestion('Sketch', 'public', null)).toBe(
      'Show “Sketch” to everyone in the server?',
    );
    expect(privacyQuestion('Sketch', 'role_gated', '@Artist')).toBe(
      'Show “Sketch” only to members with @Artist?',
    );
    expect(privacyQuestion('Sketch', 'role_gated', null)).toBe(
      'Choose the role that can see “Sketch”.',
    );
    expect(privacyQuestion('Sketch', 'creator_only', null)).toBe(
      'Hide “Sketch” from everyone except its creator?',
    );
  });
});
