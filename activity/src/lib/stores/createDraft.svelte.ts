// The "start a series" form's draft. It lives in a module, not in the form
// component, so leaving the screen (Back, a look at another series) and
// coming back finds everything as it was typed. It is cleared when the series
// is created. Nothing is written to storage: a draft does not outlive the
// Activity, which is one server's session.

import type { SeriesOptions } from '../types/api';

export interface CreateDraft {
  name: string;
  description: string;
  /** `''` until {@link seedDraft} has the server's channels. */
  channelId: string;
  cadence: string;
  privacy: string;
  /** `''` means no role chosen yet. Never defaulted: the creator picks. */
  privacyRoleId: string;
  /** Kept as typed, so a half-typed number survives; parsed on submit. */
  startDay: string;
}

function blank(): CreateDraft {
  return {
    name: '',
    description: '',
    channelId: '',
    cadence: '',
    privacy: '',
    privacyRoleId: '',
    startDay: '1',
  };
}

export const draft = $state<CreateDraft>(blank());

/** Whether anything typed is being kept (the choices all have defaults). */
export function hasDraft(): boolean {
  return draft.name.trim() !== '' || draft.description.trim() !== '';
}

/** Empties the draft: after a successful create, or on "Start over". */
export function resetDraft(): void {
  Object.assign(draft, blank());
}

/**
 * Fills in the choices that depend on the server's options, keeping every
 * value that is still valid. The channel defaults to the one leaf was opened
 * in when series may use it, else to the first allowed channel.
 */
export function seedDraft(options: SeriesOptions, launchChannelId: string | null): void {
  if (!options.channels.some((c) => c.id === draft.channelId)) {
    const launch = options.channels.find((c) => c.id === launchChannelId);
    draft.channelId = (launch ?? options.channels[0])?.id ?? '';
  }
  if (!options.cadences.includes(draft.cadence)) draft.cadence = options.cadences[0] ?? 'daily';
  if (!options.privacy_modes.includes(draft.privacy)) {
    draft.privacy = options.privacy_modes[0] ?? 'public';
  }
  // A role list that failed to load says nothing about the chosen role.
  const roleGone =
    !options.roles_unavailable && !options.roles.some((r) => r.id === draft.privacyRoleId);
  if (roleGone) draft.privacyRoleId = '';
}
