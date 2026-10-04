// What the "start a series" form checks and sends, kept apart from the
// component so every rule can be tested without rendering it.

import type { CreateDraft } from '../../stores/createDraft.svelte';
import type { CreateSeriesInput, SeriesOptions } from '../../types/api';
import {
  DAY_NUMBER_PROBLEM,
  daysAre,
  descriptionProblem,
  nameProblem,
  parseDayNumber,
  ruleCopy,
} from './formRules';

/** The fields that can carry an error, in the order they appear. */
export const CREATE_FIELDS = ['name', 'description', 'role', 'channel', 'startDay'] as const;
export type CreateField = (typeof CREATE_FIELDS)[number];

/** One sentence per field that would be refused; empty when the draft is fine. */
export function createProblems(draft: CreateDraft): Partial<Record<CreateField, string>> {
  const problems: Partial<Record<CreateField, string>> = {};
  const name = nameProblem(draft.name);
  if (name) problems.name = name;
  const description = descriptionProblem(draft.description);
  if (description) problems.description = description;
  if (draft.privacy === 'role_gated' && draft.privacyRoleId === '') {
    problems.role = ruleCopy('missing_privacy_role');
  }
  if (draft.channelId === '') problems.channel = 'Choose the channel you’ll post this series in.';
  if (parseDayNumber(draft.startDay) === null) problems.startDay = DAY_NUMBER_PROBLEM;
  return problems;
}

/** The request body for a draft that passed {@link createProblems}. */
export function createInput(draft: CreateDraft): CreateSeriesInput {
  return {
    name: draft.name.trim(),
    description: draft.description.trim(),
    channel_id: draft.channelId,
    cadence: draft.cadence,
    privacy: draft.privacy,
    privacy_role_id: draft.privacy === 'role_gated' ? draft.privacyRoleId : null,
    start_day: parseDayNumber(draft.startDay) ?? 1,
  };
}

/**
 * What a sprout server's probation means for the series being created, or
 * `null` when there is nothing to say: probation is off, or the series is
 * "Only me", which the threshold does not change.
 */
export function sproutNote(
  options: Pick<SeriesOptions, 'sprout_enabled' | 'sprout_threshold'>,
  privacy: string,
  roleName: string | null,
): string | null {
  if (!options.sprout_enabled || privacy === 'creator_only') return null;
  const until = `🌱 New series start as sprouts: only you can see this one until ${daysAre(options.sprout_threshold)} archived.`;
  if (privacy === 'public') return `${until} Then everyone in the server can.`;
  if (privacy === 'role_gated') {
    return `${until} Then members with ${roleName ? `the @${roleName} role` : 'the role you choose'} can.`;
  }
  return `${until} Then its privacy setting applies.`;
}
