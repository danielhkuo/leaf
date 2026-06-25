// Human-friendly labels for the domain enum strings the API speaks. Keeping
// them here means the create wizard, settings form, and any future surface
// phrase cadence/privacy the same way.

export function cadenceLabel(cadence: string): string {
  switch (cadence) {
    case 'daily':
      return 'Daily';
    case 'weekdays':
      return 'Weekdays';
    case 'weekly':
      return 'Weekly';
    case 'freeform':
      return 'Freeform (no schedule)';
    default:
      return cadence;
  }
}

export function privacyLabel(privacy: string): string {
  switch (privacy) {
    case 'public':
      return 'Everyone in the server';
    case 'role_gated':
      return 'Role-gated';
    case 'creator_only':
      return 'Only me';
    default:
      return privacy;
  }
}

// Maps the server's stable error codes (see leaf-server `policy_code` /
// `validation_code`) to a sentence the creator can act on. Anything unmapped
// falls back to a generic line.
const SERIES_ERRORS: Record<string, string> = {
  max_series: 'You’ve hit the limit of series you can run here.',
  account_too_new: 'Your Discord account is too new to start a series here yet.',
  membership_too_new: 'You haven’t been in this server long enough to start a series yet.',
  missing_creator_role: 'You don’t have a role that can start series here.',
  invalid_channel: 'That channel can’t host a series — pick a watched one.',
  invalid_name: 'Give the series a name between 2 and 40 characters.',
  invalid_description: 'Keep the description under 200 characters.',
  invalid_emoji: 'That emoji is too long.',
  invalid_start_day: 'The start day must be 1 or more.',
  missing_privacy_role: 'Choose a role to gate the series on.',
  invalid_reminder_time: 'Enter the reminder time as HH:MM (24-hour).',
  invalid_timezone: 'That timezone isn’t recognized.',
  reminder_time_required: 'Set a time for the reminder.',
  reminder_on_freeform: 'Freeform series can’t have reminders.',
  name_taken: 'A series with that name already exists here.',
  revoked: 'This series has been revoked and can’t be edited.',
  guild_not_setup: 'This server isn’t set up for series yet.',
};

export function seriesErrorMessage(code: string | undefined, fallback: string): string {
  return (code && SERIES_ERRORS[code]) || fallback;
}
