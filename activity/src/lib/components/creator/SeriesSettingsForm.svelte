<script lang="ts">
  // One scrollable form over an owned series' editable settings. It sends
  // only the fields that were changed (see settingsForm.ts), says whether
  // there is anything unsaved, and shows a refusal under the field it is
  // about. A revoked series is shown read-only with the reason.
  import { tick, untrack } from 'svelte';

  import type { SeriesOptions, SeriesSettings, UpdateSeriesInput } from '../../types/api';
  import { cadenceLabel, privacyLabel, reminderErrorMessage } from '../../utils/labels';
  import Callout from '../shared/Callout.svelte';
  import Button from '../ui/Button.svelte';
  import EmojiField from './EmojiField.svelte';
  import { FieldErrors, revealField } from './fieldErrors.svelte';
  import {
    channelLabel,
    charCount,
    daysAre,
    DESCRIPTION_MAX,
    fieldForCode,
    NAME_MAX,
  } from './formRules';
  import ReminderFields from './ReminderFields.svelte';
  import RoleSelect from './RoleSelect.svelte';
  import {
    canEditStartDay,
    changedFields,
    remindersOn,
    SETTINGS_FIELDS,
    settingsPatch,
    settingsProblems,
    settingsValues,
    type SettingsField,
  } from './settingsForm';

  interface Props {
    settings: SeriesSettings;
    options: SeriesOptions;
    saving: boolean;
    /** The last save went through. Shown as "Saved" until something is edited. */
    saved: boolean;
    /** Why the last save failed, as a sentence; `null` when it did not. */
    error: string | null;
    /** That failure's server code. It decides which field the sentence goes under. */
    errorCode?: string | null | undefined;
    /**
     * Saves the changed fields. Returning (a promise of) `true` says the save
     * went through and `settings` now holds what the server stored; the form
     * then shows those values.
     */
    onSave: (patch: UpdateSeriesInput) => unknown;
    /** Told whenever the form starts or stops having unsaved changes. */
    onDirtyChange?: ((dirty: boolean) => void) | undefined;
    /** Fetches the options again, for when the role list did not load. */
    onReloadOptions?: (() => void) | undefined;
    /** Whether a day is archived yet, when known (reminders start after the first). */
    hasDays?: boolean | undefined;
  }
  let {
    settings,
    options,
    saving,
    saved,
    error,
    errorCode = null,
    onSave,
    onDirtyChange,
    onReloadOptions,
    hasDays,
  }: Props = $props();

  const uid = $props.id();

  // Mounted with a loaded snapshot: the editable copy starts from it once.
  // What counts as "changed" is always measured against `settings` itself.
  // svelte-ignore state_referenced_locally
  let form = $state(settingsValues(settings));

  const readOnly = $derived(settings.state === 'revoked');
  const startDayKnown = $derived(canEditStartDay(settings));
  const dirty = $derived(!readOnly && changedFields(settings, form).length > 0);
  $effect(() => {
    onDirtyChange?.(dirty);
  });

  const descriptionCount = $derived(charCount(form.description.trim()));
  const nameCount = $derived(charCount(form.name.trim()));
  const sprout = $derived(settings.state === 'sprout');
  const gated = $derived(form.privacy === 'role_gated');
  /** The saved channel when it is no longer one the server allows for series. */
  const staleChannelId = $derived(
    settings.channel_id !== null && !options.channels.some((c) => c.id === settings.channel_id)
      ? settings.channel_id
      : null,
  );
  const reminderChannel = $derived.by(() => {
    if (form.channelId === '') return null;
    const channel = options.channels.find((c) => c.id === form.channelId);
    return channel ? channelLabel(channel) : 'the series channel';
  });
  const undelivered = $derived(
    settings.reminder_enabled && settings.reminder_error !== undefined
      ? settings.reminder_error
      : null,
  );
  const undeliveredOn = $derived(
    settings.reminder_error_at === undefined
      ? null
      : new Date(settings.reminder_error_at * 1000).toLocaleDateString(undefined, {
          day: 'numeric',
          month: 'short',
        }),
  );

  function onScreen(field: SettingsField): boolean {
    if (field === 'startDay') return startDayKnown;
    if (field === 'role') return gated && options.roles.length > 0;
    if (field === 'reminderTime' || field === 'reminderTz') return remindersOn(form);
    return true;
  }

  const errors = new FieldErrors<SettingsField>(() => {
    if (error === null) return null;
    const coded = fieldForCode(errorCode);
    const field = SETTINGS_FIELDS.find((f) => f === coded) ?? null;
    // A field that is not on screen can't carry the message.
    return { field: field !== null && onScreen(field) ? field : null, message: error };
  });

  async function focusField(field: SettingsField): Promise<void> {
    await tick();
    const control = document.getElementById(`${uid}-${field}`);
    if (control) revealField(control);
  }

  // The server refused the save: go to the field it is about.
  $effect(() => {
    if (error === null) return;
    const field = untrack(() => errors.first(SETTINGS_FIELDS));
    if (field) void focusField(field);
  });

  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    if (saving || !dirty) return;
    const problems = settingsProblems(settings, form);
    const first = SETTINGS_FIELDS.find((field) => problems[field] !== undefined);
    if (first) {
      errors.refuse(problems);
      void focusField(first);
      return;
    }
    errors.sent();
    const sent = JSON.stringify(form);
    const ok = await onSave(settingsPatch(settings, form));
    // Saved, and nothing was edited meanwhile: show what the server stored
    // (a trimmed name, a zero-padded time).
    if (ok === true && JSON.stringify(form) === sent) form = settingsValues(settings);
  }

  const status = $derived.by(() => {
    if (saving) return 'Saving…';
    if (dirty) return 'Unsaved changes';
    return saved ? 'Saved' : '';
  });

  function invalid(field: SettingsField): 'true' | undefined {
    return errors.of(field) === null ? undefined : 'true';
  }

  /** The ids describing a control: its hint (if any), then its error (if any). */
  function describedBy(field: SettingsField, hint?: string): string | undefined {
    const ids = [hint, errors.of(field) === null ? undefined : `${uid}-${field}-error`];
    return ids.filter((id) => id !== undefined).join(' ') || undefined;
  }
</script>

{#snippet problem(field: SettingsField)}
  {@const message = errors.of(field)}
  {#if message}<p class="field-error" id="{uid}-{field}-error">{message}</p>{/if}
{/snippet}

<form class="settings" novalidate aria-busy={saving} onsubmit={submit}>
  {#if readOnly}
    <Callout title="A server admin revoked this series" tone="warning">
      It’s hidden from everyone, and it can’t be changed or take new posts. Its days are kept. Ask a
      server admin to restore it.
    </Callout>
  {:else if undelivered}
    <Callout title="leaf couldn’t deliver your last reminder" tone="warning">
      {reminderErrorMessage(undelivered)}
      {#if undeliveredOn}It was tried on {undeliveredOn}.{/if}
    </Callout>
  {/if}

  <div class="panel">
    <fieldset class="fields" disabled={readOnly}>
      <div class="field">
        <label class="field-label" for="{uid}-name">Series name</label>
        <input
          id="{uid}-name"
          class="control"
          name="series-name"
          bind:value={form.name}
          required
          autocomplete="off"
          autocapitalize="sentences"
          enterkeyhint="done"
          aria-invalid={invalid('name')}
          aria-describedby={describedBy('name')}
          oninput={() => errors.edited('name')}
        />
        {#if nameCount > NAME_MAX - 10}
          <p class="count" class:over={nameCount > NAME_MAX}>
            {nameCount}/{NAME_MAX}<span class="sr-only"> characters</span>
          </p>
        {/if}
        {@render problem('name')}
      </div>

      <div class="field">
        <label class="field-label" for="{uid}-description">Description</label>
        <textarea
          id="{uid}-description"
          class="control"
          name="series-description"
          rows="3"
          bind:value={form.description}
          aria-invalid={invalid('description')}
          aria-describedby={describedBy('description', `${uid}-description-count`)}
          oninput={() => errors.edited('description')}
        ></textarea>
        <p
          class="count"
          class:over={descriptionCount > DESCRIPTION_MAX}
          id="{uid}-description-count"
        >
          {descriptionCount}/{DESCRIPTION_MAX}<span class="sr-only"> characters</span>
        </p>
        {@render problem('description')}
      </div>

      <EmojiField
        id="{uid}-emoji"
        bind:value={form.emoji}
        error={errors.of('emoji')}
        onedit={() => errors.edited('emoji')}
      />

      <div class="field">
        <label class="field-label" for="{uid}-privacy">Who can see it</label>
        <select
          id="{uid}-privacy"
          class="control"
          bind:value={form.privacy}
          aria-describedby={sprout ? `${uid}-privacy-hint` : undefined}
          onchange={() => errors.edited('role')}
        >
          {#each options.privacy_modes as mode (mode)}
            <option value={mode}>{privacyLabel(mode)}</option>
          {/each}
          {#if !options.privacy_modes.includes(settings.privacy)}
            <option value={settings.privacy}>{privacyLabel(settings.privacy)}</option>
          {/if}
        </select>
        {#if sprout}
          <p class="hint" id="{uid}-privacy-hint">
            🌱 Still a sprout: only you can see this series until {daysAre(
              options.sprout_threshold,
            )}
            archived. After that, this setting decides.
          </p>
        {/if}
      </div>

      {#if gated}
        <div class="field">
          <label class="field-label" for="{uid}-role">Role</label>
          {#if options.roles.length > 0}
            <RoleSelect
              id="{uid}-role"
              roles={options.roles}
              bind:value={form.privacyRoleId}
              invalid={errors.of('role') !== null}
              describedby={describedBy('role')}
              onchange={() => errors.edited('role')}
            />
          {:else}
            <p class="hint">
              {#if options.roles_unavailable}
                leaf couldn’t load this server’s roles, so the role can’t be changed right now.
                {#if onReloadOptions}
                  <button type="button" class="link" onclick={onReloadOptions}>
                    Load roles again
                  </button>
                {/if}
              {:else}
                This server has no roles a series can be limited to.
              {/if}
            </p>
          {/if}
          {@render problem('role')}
        </div>
      {/if}

      <div class="field">
        <label class="field-label" for="{uid}-cadence">How often you post</label>
        <select
          id="{uid}-cadence"
          class="control"
          bind:value={form.cadence}
          aria-describedby="{uid}-cadence-hint"
        >
          {#each options.cadences as cadence (cadence)}
            <option value={cadence}>{cadenceLabel(cadence)}</option>
          {/each}
          <!-- A saved value the server no longer offers still has to show. -->
          {#if !options.cadences.includes(settings.cadence)}
            <option value={settings.cadence}>{cadenceLabel(settings.cadence)}</option>
          {/if}
        </select>
        <p class="hint" id="{uid}-cadence-hint">leaf uses this for reminders.</p>
      </div>

      <div class="field">
        <label class="field-label" for="{uid}-channel">Channel</label>
        <select
          id="{uid}-channel"
          class="control"
          bind:value={form.channelId}
          aria-invalid={invalid('channel')}
          aria-describedby={describedBy('channel', `${uid}-channel-hint`)}
          onchange={() => errors.edited('channel')}
        >
          {#if form.channelId === ''}<option value="" disabled>Choose a channel…</option>{/if}
          {#if staleChannelId}
            <option value={staleChannelId}>Its current channel (no longer allowed)</option>
          {/if}
          {#each options.channels as channel (channel.id)}
            <option value={channel.id}>{channelLabel(channel)}</option>
          {/each}
        </select>
        <p class="hint" id="{uid}-channel-hint">
          Where you post this series. leaf uses it to pick the series when you archive a post from
          there, and for channel reminders.
        </p>
        {#if staleChannelId && form.channelId === staleChannelId}
          <p class="hint caution">
            This channel is no longer one this server allows for series, so posts there can’t be
            archived. Choose another channel, or ask a server admin to add it back with /setup. Your
            other settings still save.
          </p>
        {/if}
        {@render problem('channel')}
      </div>

      {#if startDayKnown}
        <div class="field">
          <label class="field-label" for="{uid}-startDay">First day number</label>
          <input
            id="{uid}-startDay"
            class="control"
            type="text"
            inputmode="numeric"
            pattern="[0-9]*"
            autocomplete="off"
            enterkeyhint="done"
            bind:value={form.startDay}
            aria-invalid={invalid('startDay')}
            aria-describedby={describedBy('startDay', `${uid}-startDay-hint`)}
            oninput={() => errors.edited('startDay')}
          />
          <p class="hint" id="{uid}-startDay-hint">
            The number this series starts counting from. Day numbers before it aren’t counted as
            skipped. It can’t be higher than the earliest day already archived.
          </p>
          {@render problem('startDay')}
        </div>
      {/if}

      <ReminderFields
        id={uid}
        bind:enabled={form.reminderEnabled}
        savedOn={remindersOn({
          reminderEnabled: settings.reminder_enabled,
          cadence: settings.cadence,
        })}
        bind:time={form.reminderTime}
        bind:timezone={form.reminderTz}
        bind:dm={form.reminderDm}
        cadence={form.cadence}
        serverZone={options.guild_timezone}
        channel={reminderChannel}
        hidden={form.privacy !== 'public' || sprout}
        {hasDays}
        timeError={errors.of('reminderTime')}
        zoneError={errors.of('reminderTz')}
        onedit={(field) => errors.edited(field)}
      />
    </fieldset>
  </div>

  {#if !readOnly}
    <div class="savebar" class:pinned={status !== ''}>
      {#if errors.form}<p class="form-error" role="alert">{errors.form}</p>{/if}
      <div class="save-row">
        <p class="status" class:ok={status === 'Saved'} role="status">{status}</p>
        <Button variant="primary" type="submit" disabled={saving || !dirty}>Save changes</Button>
      </div>
    </div>
  {/if}
</form>

<style>
  .settings {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-md);
  }
  /* Whatever is scrolled to (a control taking focus from Tab or the
   * keyboard's next key, a refused field and its message) stops clear of
   * the stuck save bar and of the top inset, not behind them. */
  .settings :global(:is(input, select, textarea, button, .field-error)) {
    scroll-margin: calc(var(--safe-top) + var(--space-xl)) 0
      calc(var(--control-height) + 3 * var(--space-sm) + var(--safe-bottom));
  }
  .panel {
    padding: var(--space-md);
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-xl);
    box-shadow: var(--shadow-card);
  }
  .fields {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-md);
    min-width: 0;
    margin: 0;
    padding: 0;
    border: 0;
  }
  .fields:disabled {
    opacity: 0.75;
  }
  .control[aria-invalid='true'] {
    border-color: var(--error-fill);
  }
  .hint,
  .count {
    margin: 0;
    font-size: var(--fs-caption);
  }
  .hint {
    color: var(--ink-muted);
  }
  .caution {
    padding-left: var(--space-sm);
    color: var(--ink);
    border-left: 2px solid var(--warning-fill);
  }
  .count {
    justify-self: end;
    color: var(--ink-subtle);
    font-variant-numeric: tabular-nums;
  }
  .field-error,
  .form-error,
  .status {
    margin: 0;
    font-size: var(--fs-body-sm);
  }
  .count.over,
  .field-error,
  .form-error {
    color: var(--error);
  }
  .count.over {
    font-weight: var(--fw-emphasis);
  }
  .link {
    min-height: var(--touch-target);
    margin-left: calc(-1 * var(--space-xs));
    padding: 0 var(--space-xs);
    color: var(--link);
    font: inherit;
    font-weight: var(--fw-emphasis);
    background: none;
    border: 0;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  /* Save sits at the end of a long form. Once there is something to say
   * (unsaved changes, saving, saved) it sticks to the bottom of the screen,
   * over the bottom inset, so Save is in reach from any field and "Saved"
   * shows where it was pressed. A view that pads its sides sets --form-bleed
   * to that padding, so the bar reaches the screen edges. */
  .savebar {
    display: grid;
    gap: var(--space-xs);
    margin: 0 calc(-1 * var(--form-bleed, 0px));
    padding: var(--space-sm) var(--form-bleed, 0px);
  }
  .savebar.pinned {
    position: sticky;
    bottom: 0;
    z-index: 1;
    margin-bottom: calc(-1 * var(--safe-bottom));
    padding-bottom: max(var(--space-sm), var(--safe-bottom));
    background: var(--canvas);
    box-shadow: 0 -1px 0 var(--hairline);
  }
  .save-row {
    display: flex;
    gap: var(--space-md);
    align-items: center;
    justify-content: space-between;
  }
  .status {
    color: var(--ink-muted);
  }
  .status.ok {
    color: var(--success);
    font-weight: var(--fw-emphasis);
  }

  @media (hover: hover) {
    .link:hover {
      text-decoration: underline;
    }
  }
</style>
