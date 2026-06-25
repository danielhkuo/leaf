<script lang="ts">
  // A single scrollable form over an owned series' editable fields. Reminders
  // are disabled (with an explanation) for freeform series, mirroring the
  // server rule. Submitting hands a partial patch to the parent.
  import type { SeriesOptions, SeriesSettings, UpdateSeriesInput } from '../../types/api';
  import { cadenceLabel, privacyLabel } from '../../utils/labels';
  import Button from '../ui/Button.svelte';

  interface Props {
    settings: SeriesSettings;
    options: SeriesOptions;
    saving: boolean;
    saved: boolean;
    error: string | null;
    onSave: (patch: UpdateSeriesInput) => void;
  }
  let { settings, options, saving, saved, error, onSave }: Props = $props();

  // Mounted fresh per series with a loaded snapshot; seeding the editable
  // fields from props once (not a missed derived) is the intent here. A later
  // PATCH replaces `settings`, but we keep the user's in-progress edits.
  // svelte-ignore state_referenced_locally
  let description = $state(settings.description);
  // svelte-ignore state_referenced_locally
  let emoji = $state(settings.emoji);
  // svelte-ignore state_referenced_locally
  let cadence = $state(settings.cadence);
  // svelte-ignore state_referenced_locally
  let privacy = $state(settings.privacy);
  // svelte-ignore state_referenced_locally
  let privacyRoleId = $state(settings.privacy_role_id ?? options.roles[0]?.id ?? '');
  // svelte-ignore state_referenced_locally
  let channelId = $state(settings.channel_id ?? options.channels[0]?.id ?? '');
  // svelte-ignore state_referenced_locally
  let passive = $state(settings.detection_mode === 'passive');
  // svelte-ignore state_referenced_locally
  let reminderEnabled = $state(settings.reminder_enabled);
  // svelte-ignore state_referenced_locally
  let reminderTime = $state(settings.reminder_time ?? '');
  // svelte-ignore state_referenced_locally
  let reminderTz = $state(settings.reminder_timezone ?? '');
  // svelte-ignore state_referenced_locally
  let reminderDm = $state(settings.reminder_dm);

  const isFreeform = $derived(cadence === 'freeform');

  function save(e: SubmitEvent): void {
    e.preventDefault();
    const patch: UpdateSeriesInput = {
      description: description.trim(),
      emoji: emoji.trim(),
      cadence,
      privacy,
      privacy_role_id: privacy === 'role_gated' ? privacyRoleId : null,
      channel_id: channelId,
      detection_mode: passive ? 'passive' : 'context_menu',
      reminder_dm: reminderDm,
    };
    if (isFreeform) {
      patch.reminder_enabled = false;
    } else {
      patch.reminder_enabled = reminderEnabled;
      if (reminderEnabled) {
        patch.reminder_time = reminderTime;
        if (reminderTz.trim()) patch.reminder_timezone = reminderTz.trim();
      }
    }
    onSave(patch);
  }
</script>

<form class="form" onsubmit={save}>
  <label class="field">
    <span class="field-label">Description</span>
    <textarea class="control" bind:value={description} maxlength="200" rows="3"></textarea>
  </label>

  <div class="two">
    <label class="field">
      <span class="field-label">Reaction emoji</span>
      <input class="control" bind:value={emoji} maxlength="8" />
    </label>
    <label class="field">
      <span class="field-label">Cadence</span>
      <select class="control" bind:value={cadence}>
        {#each options.cadences as c (c)}
          <option value={c}>{cadenceLabel(c)}</option>
        {/each}
      </select>
    </label>
  </div>

  <div class="two">
    <label class="field">
      <span class="field-label">Privacy</span>
      <select class="control" bind:value={privacy}>
        {#each options.privacy_modes as p (p)}
          <option value={p}>{privacyLabel(p)}</option>
        {/each}
      </select>
    </label>
    {#if privacy === 'role_gated'}
      <label class="field">
        <span class="field-label">Role</span>
        <select class="control" bind:value={privacyRoleId}>
          {#each options.roles as r (r.id)}
            <option value={r.id}>{r.name}</option>
          {/each}
        </select>
      </label>
    {/if}
  </div>

  <label class="field">
    <span class="field-label">Channel</span>
    <select class="control" bind:value={channelId}>
      {#each options.channels as ch (ch.id)}
        <option value={ch.id}>#{ch.name}</option>
      {/each}
    </select>
  </label>

  <label class="check">
    <input type="checkbox" bind:checked={passive} />
    <span>Passive capture — leaf offers to archive your media posts in this channel.</span>
  </label>

  <fieldset class="reminders" disabled={isFreeform}>
    <legend class="field-label">Reminders</legend>
    {#if isFreeform}
      <p class="muted">Freeform series have no schedule to remind against.</p>
    {/if}
    <label class="check">
      <input type="checkbox" bind:checked={reminderEnabled} />
      <span>Remind me when I’m behind</span>
    </label>
    {#if reminderEnabled && !isFreeform}
      <div class="two">
        <label class="field">
          <span class="field-label">Time (24h)</span>
          <input class="control" type="time" bind:value={reminderTime} />
        </label>
        <label class="field">
          <span class="field-label">Timezone <em>(optional)</em></span>
          <input class="control" bind:value={reminderTz} placeholder={options.guild_timezone} />
        </label>
      </div>
      <label class="check">
        <input type="checkbox" bind:checked={reminderDm} />
        <span>Send by DM (otherwise pings the channel)</span>
      </label>
    {/if}
  </fieldset>

  {#if error}<p class="inline-error" role="alert">{error}</p>{/if}

  <div class="actions">
    <Button variant="primary" type="submit" disabled={saving}>
      {saving ? 'Saving…' : 'Save settings'}
    </Button>
    {#if saved}<span class="ok">Saved</span>{/if}
  </div>
</form>

<style>
  .form {
    display: grid;
    gap: var(--space-md);
    padding: var(--space-md);
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-xl);
  }
  .two {
    display: grid;
    gap: var(--space-md);
    grid-template-columns: 1fr;
  }
  .field em {
    color: var(--ink-subtle);
    font-style: normal;
  }
  .check {
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    font-size: var(--fs-body-sm);
  }
  .reminders {
    display: grid;
    gap: var(--space-sm);
    margin: 0;
    padding: var(--space-sm) var(--space-md);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-lg);
  }
  .reminders[disabled] {
    opacity: 0.6;
  }
  .reminders legend {
    padding: 0 var(--space-xs);
  }
  .muted {
    margin: 0;
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }
  .inline-error {
    margin: 0;
    color: var(--error);
    font-size: var(--fs-body-sm);
  }
  .actions {
    display: flex;
    gap: var(--space-md);
    align-items: center;
  }
  .ok {
    color: var(--success);
    font-size: var(--fs-body-sm);
  }
  @media (min-width: 960px) {
    .two {
      grid-template-columns: 1fr 1fr;
    }
  }
</style>
