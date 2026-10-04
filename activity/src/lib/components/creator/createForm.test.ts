import { beforeEach, describe, expect, it } from 'vitest';

import {
  draft,
  hasDraft,
  resetDraft,
  seedDraft,
  type CreateDraft,
} from '../../stores/createDraft.svelte';
import type { SeriesOptions } from '../../types/api';
import { createInput, createProblems, sproutNote } from './createForm';

const options: SeriesOptions = {
  channels: [
    { id: 'c1', name: 'art' },
    { id: 'c2', name: 'daily' },
  ],
  roles: [{ id: 'r1', name: 'Member' }],
  cadences: ['daily', 'weekly', 'freeform'],
  privacy_modes: ['public', 'role_gated', 'creator_only'],
  guild_timezone: 'America/Chicago',
  sprout_enabled: true,
  sprout_threshold: 3,
};

function filled(extra: Partial<CreateDraft> = {}): CreateDraft {
  return {
    name: 'Morning Pages',
    description: '',
    channelId: 'c1',
    cadence: 'daily',
    privacy: 'public',
    privacyRoleId: '',
    startDay: '1',
    ...extra,
  };
}

describe('the draft store', () => {
  beforeEach(resetDraft);

  it('takes its choices from the options, the channel from where leaf was opened', () => {
    seedDraft(options, 'c2');
    expect(draft).toMatchObject({
      channelId: 'c2',
      cadence: 'daily',
      privacy: 'public',
      privacyRoleId: '',
      startDay: '1',
    });
  });

  it('falls back to the first channel when leaf was opened somewhere series can’t use', () => {
    seedDraft(options, 'elsewhere');
    expect(draft.channelId).toBe('c1');
    resetDraft();
    seedDraft(options, null);
    expect(draft.channelId).toBe('c1');
  });

  it('never picks a role for the creator', () => {
    seedDraft(options, null);
    expect(draft.privacyRoleId).toBe('');
  });

  it('keeps what was chosen earlier while it is still valid', () => {
    seedDraft(options, 'c1');
    Object.assign(draft, { name: 'Kept', channelId: 'c2', cadence: 'weekly', privacyRoleId: 'r1' });
    seedDraft(options, 'c1');
    expect(draft).toMatchObject({
      name: 'Kept',
      channelId: 'c2',
      cadence: 'weekly',
      privacyRoleId: 'r1',
    });
  });

  it('drops a channel or role the server no longer offers', () => {
    seedDraft(options, null);
    Object.assign(draft, { channelId: 'gone', privacyRoleId: 'gone' });
    seedDraft(options, null);
    expect(draft.channelId).toBe('c1');
    expect(draft.privacyRoleId).toBe('');
  });

  it('keeps the chosen role while the role list is merely unavailable', () => {
    seedDraft(options, null);
    draft.privacyRoleId = 'r1';
    seedDraft({ ...options, roles: [], roles_unavailable: true }, null);
    expect(draft.privacyRoleId).toBe('r1');
  });

  it('counts only typed text as a draft worth mentioning', () => {
    seedDraft(options, null);
    expect(hasDraft()).toBe(false);
    draft.name = ' ';
    expect(hasDraft()).toBe(false);
    draft.description = 'something';
    expect(hasDraft()).toBe(true);
    resetDraft();
    expect(hasDraft()).toBe(false);
    expect(draft.channelId).toBe('');
  });
});

describe('createProblems', () => {
  it('finds nothing wrong with a named draft on defaults', () => {
    expect(createProblems(filled())).toEqual({});
  });

  it('reports each field that would be refused', () => {
    const problems = createProblems(
      filled({
        name: '',
        description: 'x'.repeat(201),
        privacy: 'role_gated',
        channelId: '',
        startDay: '1.5',
      }),
    );
    expect(Object.keys(problems).sort()).toEqual(
      ['channel', 'description', 'name', 'role', 'startDay'].sort(),
    );
    expect(problems.startDay).toBe('Enter a whole number from 1 to 999999.');
    expect(problems.role).toBe('Choose the role that can view this series.');
  });

  it('only asks for a role when the series is role-only', () => {
    expect(createProblems(filled({ privacy: 'creator_only' })).role).toBeUndefined();
    expect(
      createProblems(filled({ privacy: 'role_gated', privacyRoleId: 'r1' })).role,
    ).toBeUndefined();
  });
});

describe('createInput', () => {
  it('trims the text and sends the role only for a role-only series', () => {
    expect(
      createInput(
        filled({
          name: '  Morning Pages ',
          description: ' hello ',
          privacyRoleId: 'r1',
          startDay: ' 200 ',
        }),
      ),
    ).toEqual({
      name: 'Morning Pages',
      description: 'hello',
      channel_id: 'c1',
      cadence: 'daily',
      privacy: 'public',
      privacy_role_id: null,
      start_day: 200,
    });
    expect(
      createInput(filled({ privacy: 'role_gated', privacyRoleId: 'r1' })).privacy_role_id,
    ).toBe('r1');
  });
});

describe('sproutNote', () => {
  it('says who sees the series after the threshold, by privacy', () => {
    expect(sproutNote(options, 'public', null)).toMatch(
      /only you can see this one until 3 days are archived\. Then everyone in the server can\.$/,
    );
    expect(sproutNote(options, 'role_gated', 'Artists')).toMatch(
      /Then members with the @Artists role can\.$/,
    );
    expect(sproutNote(options, 'role_gated', null)).toMatch(/the role you choose can\.$/);
  });

  it('never tells an "Only me" series it will go public', () => {
    expect(sproutNote(options, 'creator_only', null)).toBeNull();
  });

  it('says nothing where probation is off', () => {
    expect(sproutNote({ sprout_enabled: false, sprout_threshold: 3 }, 'public', null)).toBeNull();
  });

  it('agrees with a threshold of one', () => {
    expect(sproutNote({ sprout_enabled: true, sprout_threshold: 1 }, 'public', null)).toContain(
      'until 1 day is archived',
    );
  });
});
