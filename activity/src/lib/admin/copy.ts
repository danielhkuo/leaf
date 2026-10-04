// Everything the admin panel says about a failure, a sign-in problem or a
// series' privacy and state lives here, so the picker, the settings form and
// the series rows phrase the same thing the same way.

import { privacyLabel } from '../utils/labels';
import { AdminApiError } from './client';
import type { AdminSeries } from './schemas';

/** What a failed call was about; decides the wording of a 404. */
export type ErrorSubject = 'server' | 'series';

// One sentence per server code the admin API answers with (section 5.5 of
// docs/ux-audit.md). `invalid_limit` has none: the server's own sentence
// names the number that is out of range.
const CODE_COPY: Record<string, string> = {
  invalid_timezone: 'leaf doesn’t know that timezone. Choose another one.',
  unknown_role: 'leaf can’t find that role in this server. Pick another one.',
  unknown_channel: 'leaf can’t find that channel in this server. Pick another one.',
  role_required: 'Choose the role that can see this series.',
  discord_unavailable: 'leaf can’t reach Discord right now. Try again in a moment.',
};

/**
 * One sentence an admin can act on, for any thrown value. A known server code
 * wins, then the server's own sentence for a refused value, then copy for the
 * kind of failure.
 */
export function adminErrorMessage(e: unknown, subject: ErrorSubject = 'server'): string {
  if (!(e instanceof AdminApiError)) return 'Something went wrong. Try again.';
  if (e.code !== undefined && Object.prototype.hasOwnProperty.call(CODE_COPY, e.code)) {
    const known = CODE_COPY[e.code];
    if (known !== undefined) return known;
  }
  switch (e.kind) {
    case 'network':
      return 'Can’t reach leaf. Check your connection and try again.';
    case 'timeout':
      return 'leaf is taking too long to answer. Try again in a moment.';
    case 'unauthorized':
      return 'Your admin session has expired. Sign in again.';
    case 'forbidden':
    case 'not_found':
      return subject === 'series'
        ? 'leaf can’t find this series any more. Reload the page to see the current list.'
        : 'This server is no longer available to you. Sign in again to refresh your server list.';
    case 'server':
      return 'leaf had a problem. Try again in a moment.';
    case 'bad_response':
      return 'leaf sent something this page can’t read. Reload the page. If that doesn’t help, leaf may need updating.';
    case 'rejected':
      return e.detail ?? 'leaf wouldn’t accept that. Check the values and try again.';
  }
}

/** Whether trying the same call again can help (so a Retry button makes sense). */
export function canRetry(e: unknown): boolean {
  return e instanceof AdminApiError ? e.retryable : true;
}

/** Whether the failure means this sign-in can no longer manage the server. */
export function isGone(e: unknown): boolean {
  return e instanceof AdminApiError && (e.kind === 'not_found' || e.kind === 'forbidden');
}

/**
 * Whether a failed change may have gone through all the same: no answer
 * came, or none that settles it. Only a refusal (a 4xx) is certain, so the
 * panel must not say "wasn't saved" for these before it has looked.
 */
export function outcomeUnknown(e: unknown): boolean {
  if (!(e instanceof AdminApiError)) return true;
  return (
    e.kind === 'network' || e.kind === 'timeout' || e.kind === 'server' || e.kind === 'bad_response'
  );
}

/**
 * For a change to a series that leaf neither confirmed nor refused, when the
 * series could not be read again either. It never says the change failed.
 * The row offers "Check again" under it.
 */
export function unconfirmedMessage(e: unknown): string {
  const unknown = 'can’t tell whether the change went through';
  switch (e instanceof AdminApiError ? e.kind : undefined) {
    case 'network':
      return `This page can’t reach leaf, so it ${unknown}. Check your connection, then check again.`;
    case 'timeout':
      return `leaf didn’t answer in time, so this page ${unknown}. Check again in a moment.`;
    case 'bad_response':
      return `leaf sent something this page can’t read, so it ${unknown}. Check again, or reload the page.`;
    case 'server':
      return `leaf had a problem, so this page ${unknown}. Check again in a moment.`;
    default:
      return `Something went wrong, so this page ${unknown}. Check again.`;
  }
}

/** Why the admin is looking at the sign-in card instead of the panel. */
export interface SignInProblem {
  title: string;
  message: string;
}

/**
 * Copy for the codes the OAuth callback redirects with (`/admin#error=<code>`).
 * `origin` is this page's origin, for the redirect the Developer Portal needs.
 */
export function signInProblem(code: string, origin: string): SignInProblem {
  switch (code) {
    case 'denied':
      return {
        title: 'Sign-in cancelled',
        message: 'You cancelled the Discord sign-in, so nothing changed.',
      };
    case 'expired':
      return {
        title: 'That sign-in took too long',
        message: 'A sign-in has to finish within 10 minutes. Start it again.',
      };
    case 'exchange_failed':
      return {
        title: 'Discord rejected the sign-in',
        message: `If it keeps happening, whoever runs leaf should check that ${origin}/admin/callback is listed under OAuth2 → Redirects in the Discord Developer Portal.`,
      };
    case 'discord_unavailable':
      return {
        title: 'Discord isn’t answering',
        message: 'leaf couldn’t reach Discord to finish signing you in. Try again in a moment.',
      };
    case 'no_guilds':
      return NO_GUILDS;
    default:
      return {
        title: 'Sign-in didn’t finish',
        message: 'Something went wrong while signing you in. Try again.',
      };
  }
}

/** The account is signed in but has nothing to manage here. */
export const NO_GUILDS: SignInProblem = {
  title: 'No server to manage',
  message:
    'No server you manage has leaf in it. You need the Manage Server permission, and leaf’s bot has to be in that server (and have been online there once). If you’re on the wrong Discord account, switch accounts on discord.com, then sign in again.',
};

/** The token ran out, or the server stopped accepting it. */
export function sessionExpired(draftKept: boolean): SignInProblem {
  return {
    title: 'Your session expired',
    message: draftKept
      ? 'Sign in again to carry on. Your unsaved settings are kept and will be back when you return.'
      : 'Admin sign-ins last an hour. Sign in again to carry on.',
  };
}

/**
 * Who can see a series, in the words the gallery's own settings use. The one
 * difference: the gallery says "Only me" to the creator, which would be wrong
 * on an admin's screen.
 */
export function adminPrivacyLabel(privacy: string): string {
  return privacy === 'creator_only' ? 'Only its creator' : privacyLabel(privacy);
}

/** The privacy settings a series can have, in the order the select offers them. */
export const PRIVACY_MODES: readonly string[] = ['public', 'role_gated', 'creator_only'];

/** The last four digits of an id, to tell two unnamed things apart. */
export function idTail(id: string): string {
  return `…${id.slice(-4)}`;
}

/** "by Mika", or the id when Discord no longer tells leaf the name. */
export function creatorLabel(series: Pick<AdminSeries, 'creator_id' | 'creator_name'>): string {
  return series.creator_name ? `by ${series.creator_name}` : `by member ${series.creator_id}`;
}

/** "1 day" / "3 days". */
function days(n: number): string {
  return n === 1 ? '1 day' : `${n} days`;
}

/**
 * A series' state as the row shows it. A sprout says how far along it is
 * when the server reports its archived days.
 */
export function stateLabel(
  series: Pick<AdminSeries, 'state' | 'archived_days'>,
  threshold: number,
): string {
  switch (series.state) {
    case 'active':
      return 'Active';
    case 'revoked':
      return 'Revoked';
    case 'sprout':
      return series.archived_days === undefined
        ? '🌱 Sprout'
        : `🌱 Sprout · ${series.archived_days} of ${days(threshold)} archived`;
    default:
      return series.state;
  }
}

/** What a privacy change will do, as the question the row asks before saving. */
export function privacyQuestion(name: string, privacy: string, roleName: string | null): string {
  switch (privacy) {
    case 'public':
      return `Show “${name}” to everyone in the server?`;
    case 'role_gated':
      return roleName
        ? `Show “${name}” only to members with ${roleName}?`
        : `Choose the role that can see “${name}”.`;
    case 'creator_only':
      return `Hide “${name}” from everyone except its creator?`;
    default:
      return `Change who can see “${name}”?`;
  }
}
