<script lang="ts">
  // The server's settings as one form. It measures every field against what
  // is saved, sends only what changed (see settingsForm.ts), shows a refusal
  // under the field it is about and next to Save, and reports its unsaved
  // fields upward so they can be kept across a sign-in.
  import { tick } from 'svelte';

  import { AdminApiError, isUnauthorized } from '../../lib/admin/client';
  import { adminErrorMessage, idTail, outcomeUnknown } from '../../lib/admin/copy';
  import type { AdminOptions, AdminSettings, SettingsPatch } from '../../lib/admin/schemas';
  import {
    changedFields,
    draftOf,
    fieldForCode,
    savedText,
    SETTINGS_FIELDS,
    settingsPatch,
    settingsProblems,
    settingsValues,
    withDraft,
    type SettingsDraft,
    type SettingsField,
  } from '../../lib/admin/settingsForm';
  import {
    FieldErrors,
    revealField,
    type ServerFailure,
  } from '../../lib/components/creator/fieldErrors.svelte';
  import Callout from '../../lib/components/shared/Callout.svelte';
  import Button from '../../lib/components/ui/Button.svelte';
  import { isKnownTimezone, timezoneOffsetLabel } from '../../lib/utils/timezones';
  import IdPicker, { type Choice } from './IdPicker.svelte';
  import ZoneField from './ZoneField.svelte';

  interface Props {
    /** What is saved now: the baseline every edit is measured against. */
    settings: AdminSettings;
    /** `undefined` while the pickers' lists load; `null` when leaf could not get them. */
    options: AdminOptions | null | undefined;
    /** Unsaved fields kept from before a sign-in, laid over `settings` once. */
    restored?: SettingsDraft | null;
    /** Sends the changed fields; resolves to what the server stored. */
    save: (patch: SettingsPatch) => Promise<AdminSettings>;
    /** A save went through. The parent must hand the result back as `settings`. */
    onSaved: (updated: AdminSettings) => void;
    /** Told the unsaved fields whenever they change; `null` when there are none. */
    onDraft?: ((draft: SettingsDraft | null) => void) | undefined;
    /** The server refused the admin token. */
    onUnauthorized: () => void;
    /** Fetches the pickers' lists again. */
    onReloadOptions?: (() => void) | undefined;
  }
  let {
    settings,
    options,
    restored = null,
    save,
    onSaved,
    onDraft,
    onUnauthorized,
    onReloadOptions,
  }: Props = $props();

  const uid = $props.id();

  // Mounted with loaded settings: the editable copy starts from them (and
  // from a kept draft) once. "Changed" is always measured against `settings`.
  // svelte-ignore state_referenced_locally
  let form = $state(withDraft(settings, restored));
  // Not worth a notice when what was kept is what the server holds by now.
  // svelte-ignore state_referenced_locally
  let showRestored = $state(restored !== null && changedFields(settings, form).length > 0);
  let saving = $state(false);
  /** "Saved", until the next edit. */
  let outcome = $state('');
  let failure = $state<ServerFailure<SettingsField> | null>(null);
  /** The failed save got no answer either way, so leaf may have stored it. */
  let unconfirmed = $state(false);

  const errors = new FieldErrors<SettingsField>(() => failure);

  const dirty = $derived(changedFields(settings, form).length > 0);
  $effect(() => {
    onDraft?.(draftOf(settings, form));
  });

  const roles = $derived.by((): Choice[] | null | undefined => {
    if (options === undefined) return undefined;
    if (options === null || options.roles === undefined || options.roles_unavailable) return null;
    return options.roles.map((role) => ({ id: role.id, label: `@${role.name}` }));
  });
  const channels = $derived.by((): Choice[] | null | undefined => {
    if (options === undefined) return undefined;
    if (options === null || options.channels === undefined || options.channels_unavailable) {
      return null;
    }
    return options.channels.map((channel) => ({
      id: channel.id,
      label: channel.name ? `#${channel.name}` : `A channel leaf can’t see (${idTail(channel.id)})`,
    }));
  });

  const zoneHint = $derived.by(() => {
    const offset = timezoneOffsetLabel(form.timezone);
    const now = offset ? ` Right now that is ${offset}.` : '';
    return `Sets the dates on the gallery calendar and the clock reminders run on, unless a creator picks their own.${now}`;
  });
  // A saved zone this browser can't place. leaf may still know it (an older
  // browser lacks newer names), so the caution says only what the page sees.
  const unknownZone = $derived(
    form.timezone === settings.timezone && !isKnownTimezone(settings.timezone),
  );

  const status = $derived.by(() => {
    if (saving) return 'Saving…';
    if (dirty) return 'Unsaved changes';
    return outcome;
  });
  const flagged = $derived(SETTINGS_FIELDS.filter((field) => errors.of(field) !== null).length);
  /** What went wrong, where Save was pressed. The detail is under each field. */
  const barError = $derived.by(() => {
    if (errors.form !== null) {
      // Saving again is safe either way: it sends the same values.
      return `${unconfirmed ? 'Save not confirmed.' : 'Not saved.'} ${errors.form}`;
    }
    if (flagged === 0) return null;
    return flagged === 1
      ? 'Not saved. One setting above needs a change.'
      : `Not saved. ${flagged} settings above need a change.`;
  });

  async function focusField(field: SettingsField): Promise<void> {
    await tick();
    const control = document.getElementById(`${uid}-${field}`);
    if (control) revealField(control);
  }

  function edited(field: SettingsField): void {
    errors.edited(field);
    outcome = '';
  }

  async function submit(e: SubmitEvent): Promise<void> {
    e.preventDefault();
    if (saving || !dirty) return;
    const problems = settingsProblems(settings, form, {
      roles: roles?.map((role) => role.id),
      channels: channels?.map((channel) => channel.id),
    });
    const first = SETTINGS_FIELDS.find((field) => problems[field] !== undefined);
    if (first) {
      failure = null;
      errors.refuse(problems);
      void focusField(first);
      return;
    }
    errors.sent();
    failure = null;
    outcome = '';
    saving = true;
    const sent = JSON.stringify(form);
    try {
      const updated = await save(settingsPatch(settings, form));
      onSaved(updated);
      // Nothing was edited meanwhile: show what the server stored (a zone in
      // its canonical spelling, numbers without stray zeros).
      if (JSON.stringify(form) === sent) form = settingsValues(updated);
      showRestored = false;
      outcome = savedText(updated);
    } catch (err) {
      if (isUnauthorized(err)) {
        onUnauthorized();
        return;
      }
      const field = fieldForCode(err instanceof AdminApiError ? err.code : undefined);
      failure = { field, message: adminErrorMessage(err) };
      unconfirmed = outcomeUnknown(err);
      if (field) void focusField(field);
    } finally {
      saving = false;
    }
  }

  function discardRestored(): void {
    form = settingsValues(settings);
    showRestored = false;
    failure = null;
    errors.clear();
  }

  function invalid(field: SettingsField): boolean {
    return errors.of(field) !== null;
  }

  /** The ids describing a control: its hint, then its error (if any). */
  function describedBy(field: SettingsField): string {
    const hint = `${uid}-${field}-hint`;
    return errors.of(field) === null ? hint : `${hint} ${uid}-${field}-error`;
  }
</script>

{#snippet problem(field: SettingsField)}
  {@const message = errors.of(field)}
  {#if message}<p class="field-error" id="{uid}-{field}-error">{message}</p>{/if}
{/snippet}

{#snippet reload(what: string)}
  {#if onReloadOptions}
    <button type="button" class="link" onclick={onReloadOptions}>Load the {what} again</button>
  {/if}
{/snippet}

{#snippet number(field: 'maxSeries' | 'minAccountAge' | 'minMembershipAge' | 'sproutThreshold')}
  <input
    id="{uid}-{field}"
    class="control"
    type="text"
    inputmode="numeric"
    pattern="[0-9]*"
    autocomplete="off"
    enterkeyhint="done"
    required
    bind:value={form[field]}
    disabled={field === 'sproutThreshold' && !form.sproutEnabled}
    aria-invalid={invalid(field) ? 'true' : undefined}
    aria-describedby={describedBy(field)}
    oninput={() => edited(field)}
  />
{/snippet}

<form class="settings" novalidate aria-busy={saving} onsubmit={submit}>
  {#if showRestored}
    <Callout title="Your unsaved changes are back">
      They were kept when your session expired. Check them, then save.
      {#snippet action()}
        <Button size="sm" onclick={discardRestored}>Discard them</Button>
      {/snippet}
    </Callout>
  {/if}

  <div class="panel">
    <div class="field">
      <label class="field-label" for="{uid}-timezone">Timezone</label>
      <ZoneField
        id="{uid}-timezone"
        bind:value={form.timezone}
        stored={settings.timezone}
        invalid={invalid('timezone')}
        describedby={describedBy('timezone')}
        onedit={() => edited('timezone')}
      />
      <p class="hint" id="{uid}-timezone-hint">{zoneHint}</p>
      {#if unknownZone}
        <p class="hint caution">
          “{settings.timezone}” isn’t a timezone this browser knows. If leaf doesn’t know it either,
          this server has been running on UTC: choose the right one and save.
        </p>
      {/if}
      {@render problem('timezone')}
    </div>

    <div class="field">
      <label class="field-label" for="{uid}-creatorRoleId">Creator role</label>
      <IdPicker
        id="{uid}-creatorRoleId"
        choices={roles}
        bind:value={form.creatorRoleId}
        stored={settings.creator_role_id ?? ''}
        noneLabel="Anyone can start a series"
        unlisted={(id) => `A role leaf can’t see (${idTail(id)})`}
        idHint="Role ID"
        invalid={invalid('creatorRoleId')}
        describedby={describedBy('creatorRoleId')}
        onedit={() => edited('creatorRoleId')}
      />
      <p class="hint" id="{uid}-creatorRoleId-hint">
        {#if roles === null}
          leaf couldn’t load this server’s roles. Paste a role ID, or leave this blank to let anyone
          start a series. To copy an ID, turn on Developer Mode in Discord’s settings, then
          right-click or long-press the role.
        {:else}
          Only members with this role can start a series. Series that already exist keep working.
        {/if}
      </p>
      {#if roles === null}{@render reload('roles')}{/if}
      {@render problem('creatorRoleId')}
    </div>

    <div class="field">
      <label class="field-label" for="{uid}-logChannelId">Log channel</label>
      <IdPicker
        id="{uid}-logChannelId"
        choices={channels}
        bind:value={form.logChannelId}
        stored={settings.log_channel_id ?? ''}
        noneLabel="No log"
        unlisted={(id) => `A channel leaf can’t see (${idTail(id)})`}
        idHint="Channel ID"
        invalid={invalid('logChannelId')}
        describedby={describedBy('logChannelId')}
        onedit={() => edited('logChannelId')}
      />
      <p class="hint" id="{uid}-logChannelId-hint">
        {#if channels === null}
          leaf couldn’t load this server’s channels. Paste a channel ID, or leave this blank for no
          log. To copy an ID, turn on Developer Mode in Discord’s settings, then right-click or
          long-press the channel.
        {:else}
          leaf posts a short, silent line here when a day is archived, changed or removed, and when
          a series is published, revoked or restored.
        {/if}
      </p>
      {#if channels === null}{@render reload('channels')}{/if}
      {@render problem('logChannelId')}
    </div>

    <div class="field">
      <label class="field-label" for="{uid}-maxSeries">Series per member</label>
      {@render number('maxSeries')}
      <p class="hint" id="{uid}-maxSeries-hint">
        How many series one member can have at a time. At least 1.
      </p>
      {@render problem('maxSeries')}
    </div>

    <div class="field">
      <label class="field-label" for="{uid}-minAccountAge">
        Minimum Discord account age, in days
      </label>
      {@render number('minAccountAge')}
      <p class="hint" id="{uid}-minAccountAge-hint">
        Members with a newer Discord account can’t start a series. 0 turns this off.
      </p>
      {@render problem('minAccountAge')}
    </div>

    <div class="field">
      <label class="field-label" for="{uid}-minMembershipAge">
        Minimum time in this server, in days
      </label>
      {@render number('minMembershipAge')}
      <p class="hint" id="{uid}-minMembershipAge-hint">
        Members who joined more recently can’t start a series. 0 turns this off.
      </p>
      {@render problem('minMembershipAge')}
    </div>

    <div class="field wide">
      <label class="check" for="{uid}-sproutEnabled">
        <input
          id="{uid}-sproutEnabled"
          type="checkbox"
          bind:checked={form.sproutEnabled}
          aria-describedby="{uid}-sproutEnabled-hint"
          onchange={() => edited('sproutEnabled')}
        />
        <span>Sprout stage for new series</span>
      </label>
      <p class="hint" id="{uid}-sproutEnabled-hint">
        A new series stays hidden from other members as a 🌱 sprout until it has enough archived
        days, then it appears in the gallery by itself. Turning this off publishes the series that
        are still sprouts.
      </p>
    </div>

    <div class="field">
      <label class="field-label" for="{uid}-sproutThreshold"
        >Days before a sprout is published</label
      >
      {@render number('sproutThreshold')}
      <p class="hint" id="{uid}-sproutThreshold-hint">
        {#if form.sproutEnabled}
          How many archived days a new series needs. Lowering it publishes sprouts that already have
          enough.
        {:else}
          Only used while the sprout stage is on.
        {/if}
      </p>
      {@render problem('sproutThreshold')}
    </div>
  </div>

  <div class="savebar" class:pinned={status !== '' || barError !== null}>
    {#if barError}<p class="form-error" role="alert">{barError}</p>{/if}
    <div class="save-row">
      <p class="status" class:ok={!saving && !dirty && outcome !== ''} role="status">{status}</p>
      <Button variant="primary" type="submit" disabled={saving || !dirty}>Save changes</Button>
    </div>
  </div>
</form>

<style>
  .settings {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-md);
  }
  /* Whatever is scrolled to (a control taking focus, a refused field and its
   * message) stops clear of the page's sticky header and of the stuck save
   * bar, not behind them. */
  .settings :global(:is(input, select, button, .field-error)) {
    scroll-margin: calc(var(--safe-top) + var(--appbar-h) + var(--space-xl)) 0
      calc(var(--control-height) + 3 * var(--space-sm) + var(--safe-bottom));
  }
  .panel {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-md);
    padding: var(--space-md);
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-xl);
    box-shadow: var(--shadow-card);
  }
  .field {
    align-content: start;
    min-width: 0;
  }
  .control[aria-invalid='true'] {
    border-color: var(--error-fill);
  }
  .control:disabled {
    opacity: 0.6;
  }
  .check {
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    min-height: var(--touch-target);
    color: var(--ink);
    font-size: var(--fs-body);
    font-weight: var(--fw-emphasis);
    cursor: pointer;
  }
  .hint,
  .field-error,
  .form-error,
  .status {
    margin: 0;
  }
  .hint {
    color: var(--ink-muted);
    font-size: var(--fs-caption);
  }
  .caution {
    padding-left: var(--space-sm);
    color: var(--ink);
    border-left: 2px solid var(--warning-fill);
  }
  .field-error,
  .form-error,
  .status {
    font-size: var(--fs-body-sm);
  }
  .field-error,
  .form-error {
    color: var(--error);
  }
  .link {
    justify-self: start;
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
   * (unsaved changes, saving, saved, not saved) it sticks to the bottom of
   * the screen, so Save and its outcome are in reach from any field. The
   * page sets --form-bleed to its side padding so the bar reaches the edges. */
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

  @media (min-width: 640px) {
    .panel {
      grid-template-columns: repeat(2, minmax(0, 1fr));
      column-gap: var(--space-lg);
      padding: var(--space-lg);
    }
    .wide {
      grid-column: 1 / -1;
    }
  }
  @media (hover: hover) {
    .link:hover {
      text-decoration: underline;
    }
  }
</style>
