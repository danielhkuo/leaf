<script lang="ts">
  // The "start a series" form: one screen, one required field. Every choice
  // has a default, and the ones that only matter later (channel, cadence,
  // first day number) wait under "More options". The draft lives in a module
  // store, so leaving the screen and coming back loses nothing. Submitting
  // re-runs the server's policy (the real gate); a refusal is shown under
  // the field it is about and goes away when that field is edited.
  //
  // (Still named CreateWizard: it replaced the six-step wizard in place.)
  import { tick, untrack } from 'svelte';

  import { draft, hasDraft, resetDraft, seedDraft } from '../../stores/createDraft.svelte';
  import type { CreateSeriesInput, SeriesOptions } from '../../types/api';
  import { cadenceLabel, privacyLabel } from '../../utils/labels';
  import Callout from '../shared/Callout.svelte';
  import Button from '../ui/Button.svelte';
  import {
    CREATE_FIELDS,
    createInput,
    createProblems,
    sproutNote,
    type CreateField,
  } from './createForm';
  import { FieldErrors, revealField } from './fieldErrors.svelte';
  import {
    channelLabel,
    charCount,
    DESCRIPTION_MAX,
    fieldForCode,
    NAME_MAX,
    parseDayNumber,
  } from './formRules';
  import RoleSelect from './RoleSelect.svelte';

  interface Props {
    options: SeriesOptions;
    submitting: boolean;
    /** Why the last submit failed, as a sentence; `null` when it did not. */
    error: string | null;
    /** That failure's server code. It decides which field the sentence goes under. */
    errorCode?: string | null | undefined;
    onSubmit: (input: CreateSeriesInput) => void;
    /** The channel leaf was opened in: preselected when series may use it. */
    channelId?: string | null;
    /** Fetches the options again, for when the role list did not load. */
    onReloadOptions?: (() => void) | undefined;
  }
  let {
    options,
    submitting,
    error,
    errorCode = null,
    onSubmit,
    channelId = null,
    onReloadOptions,
  }: Props = $props();

  const uid = $props.id();

  // Read before seeding: whether this visit picks up something typed earlier.
  let restored = $state(hasDraft());
  // The form mounts with the loaded options, so filling the draft's choices
  // from them once is the intent (not a missed derived).
  seedDraft(
    untrack(() => options),
    untrack(() => channelId),
  );

  const showChannel = $derived(options.channels.length > 1);
  /** Nothing can be created until an admin has chosen the series channels. */
  const noChannels = $derived(options.channels.length === 0);
  const gateOffered = $derived(options.roles.length > 0);

  const errors = new FieldErrors<CreateField>(() => {
    if (error === null) return null;
    const coded = fieldForCode(errorCode);
    const field = CREATE_FIELDS.find((f) => f === coded) ?? null;
    // A field that is not on screen can't carry the message.
    return { field: field === 'channel' && !showChannel ? null : field, message: error };
  });

  const nameCount = $derived(charCount(draft.name.trim()));
  const descriptionCount = $derived(charCount(draft.description.trim()));
  const chosenChannel = $derived(options.channels.find((c) => c.id === draft.channelId) ?? null);
  const roleName = $derived(options.roles.find((r) => r.id === draft.privacyRoleId)?.name ?? null);
  const note = $derived(sproutNote(options, draft.privacy, roleName));

  // "More options" starts open when the channel is only a guess: several are
  // allowed and leaf was not opened in one of them.
  let moreOpen = $state(untrack(() => showChannel && draft.channelId !== channelId));
  /** What the closed disclosure is currently set to, so defaults aren't hidden. */
  const moreValues = $derived(
    [
      showChannel && chosenChannel ? channelLabel(chosenChannel) : null,
      cadenceLabel(draft.cadence),
      `starts at Day ${parseDayNumber(draft.startDay) ?? '?'}`,
    ]
      .filter((part) => part !== null)
      .join(' · '),
  );

  let root = $state<HTMLFormElement>();
  const IN_MORE: readonly CreateField[] = ['channel', 'startDay'];

  /** Opens whatever hides the field, then moves focus (and the page) to it. */
  async function focusField(field: CreateField): Promise<void> {
    if (IN_MORE.includes(field)) moreOpen = true;
    await tick();
    const control =
      document.getElementById(`${uid}-${field}`) ??
      // The role select is missing while the role list is unavailable.
      root?.querySelector<HTMLElement>('input[type="radio"]:checked');
    if (control) revealField(control);
  }

  // The server refused the submit: go to the field it is about.
  $effect(() => {
    if (error === null) return;
    const field = untrack(() => errors.first(CREATE_FIELDS));
    if (field) void focusField(field);
  });

  function submit(e: SubmitEvent): void {
    e.preventDefault();
    if (submitting || noChannels) return;
    const problems = createProblems(draft);
    const first = CREATE_FIELDS.find((field) => problems[field] !== undefined);
    if (first) {
      errors.refuse(problems);
      void focusField(first);
      return;
    }
    errors.sent();
    onSubmit(createInput(draft));
  }

  /**
   * Return in a text field ends the typing; it does not create the series.
   * A series can't be deleted and counts against a small per-member limit,
   * so creating one takes a deliberate press. Focus goes to the button (which
   * also puts a phone's keyboard away): Return again, or a tap, starts it.
   */
  function doneTyping(e: KeyboardEvent): void {
    if (e.key !== 'Enter' || e.isComposing) return;
    e.preventDefault();
    root?.querySelector<HTMLButtonElement>('button[type="submit"]')?.focus();
  }

  function startOver(): void {
    resetDraft();
    seedDraft(options, channelId);
    errors.clear();
    restored = false;
    void focusField('name');
  }

  function invalid(field: CreateField): 'true' | undefined {
    return errors.of(field) === null ? undefined : 'true';
  }

  /** The ids describing a control: its hint (if any), then its error (if any). */
  function describedBy(field: CreateField, hint?: string): string | undefined {
    const ids = [hint, errors.of(field) === null ? undefined : `${uid}-${field}-error`];
    return ids.filter((id) => id !== undefined).join(' ') || undefined;
  }
</script>

{#snippet problem(field: CreateField)}
  {@const message = errors.of(field)}
  {#if message}<p class="field-error" id="{uid}-{field}-error">{message}</p>{/if}
{/snippet}

<form class="create" novalidate aria-busy={submitting} onsubmit={submit} bind:this={root}>
  {#if restored}
    <p class="restored">
      <span>Your draft from earlier is still here.</span>
      <button type="button" class="link" onclick={startOver}>Start over</button>
    </p>
  {/if}

  {#if noChannels}
    <Callout title="No channel is set up for series yet" tone="warning">
      A server admin needs to choose this server’s series channels by running /setup in chat. After
      that you can start a series here.
    </Callout>
  {/if}

  <div class="panel">
    <div class="field">
      <label class="field-label" for="{uid}-name">Series name</label>
      <input
        id="{uid}-name"
        class="control"
        name="series-name"
        bind:value={draft.name}
        required
        placeholder="Daily sketch"
        autocomplete="off"
        autocapitalize="sentences"
        enterkeyhint="done"
        aria-invalid={invalid('name')}
        aria-describedby={describedBy('name')}
        oninput={() => errors.edited('name')}
        onkeydown={doneTyping}
      />
      {#if nameCount > NAME_MAX - 10}
        <p class="count" class:over={nameCount > NAME_MAX}>
          {nameCount}/{NAME_MAX}<span class="sr-only"> characters</span>
        </p>
      {/if}
      {@render problem('name')}
    </div>

    <div class="field">
      <label class="field-label" for="{uid}-description">
        Description <span class="optional">(optional)</span>
      </label>
      <textarea
        id="{uid}-description"
        class="control"
        name="series-description"
        rows="2"
        bind:value={draft.description}
        aria-invalid={invalid('description')}
        aria-describedby={describedBy('description', `${uid}-description-count`)}
        oninput={() => errors.edited('description')}
      ></textarea>
      <p class="count" class:over={descriptionCount > DESCRIPTION_MAX} id="{uid}-description-count">
        {descriptionCount}/{DESCRIPTION_MAX}<span class="sr-only"> characters</span>
      </p>
      {@render problem('description')}
    </div>

    <fieldset class="group">
      <legend>Who can see it?</legend>
      {#each options.privacy_modes as mode (mode)}
        <label class="choice">
          <input
            type="radio"
            name="{uid}-privacy"
            value={mode}
            bind:group={draft.privacy}
            disabled={mode === 'role_gated' && !gateOffered && draft.privacy !== mode}
            onchange={() => errors.edited('role')}
          />
          <span>{privacyLabel(mode)}</span>
        </label>
      {/each}
      {#if gateOffered && draft.privacy === 'role_gated'}
        <div class="field">
          <label class="field-label" for="{uid}-role">Role</label>
          <RoleSelect
            id="{uid}-role"
            roles={options.roles}
            bind:value={draft.privacyRoleId}
            invalid={errors.of('role') !== null}
            describedby={describedBy('role')}
            onchange={() => errors.edited('role')}
          />
        </div>
      {:else if !gateOffered && options.privacy_modes.includes('role_gated')}
        <p class="hint">
          {#if options.roles_unavailable}
            leaf couldn’t load this server’s roles, so “{privacyLabel('role_gated')}” can’t be
            chosen right now.
            {#if onReloadOptions}
              <button type="button" class="link" onclick={onReloadOptions}>Load roles again</button>
            {/if}
          {:else}
            This server has no roles a series can be limited to.
          {/if}
        </p>
      {/if}
      {@render problem('role')}
    </fieldset>

    <details class="more" bind:open={moreOpen}>
      <summary>
        <span class="more-title">More options</span>
        <span class="more-values">{moreValues}</span>
      </summary>
      <div class="more-body">
        {#if showChannel}
          <div class="field">
            <label class="field-label" for="{uid}-channel">Channel</label>
            <select
              id="{uid}-channel"
              class="control"
              bind:value={draft.channelId}
              aria-invalid={invalid('channel')}
              aria-describedby={describedBy('channel', `${uid}-channel-hint`)}
              onchange={() => errors.edited('channel')}
            >
              {#each options.channels as channel (channel.id)}
                <option value={channel.id}>{channelLabel(channel)}</option>
              {/each}
            </select>
            <p class="hint" id="{uid}-channel-hint">
              The channel you’ll post this series in. leaf uses it to pick the series when you
              archive a post from there, and for channel reminders.
            </p>
            {@render problem('channel')}
          </div>
        {/if}

        <div class="field">
          <label class="field-label" for="{uid}-cadence">How often you’ll post</label>
          <select
            id="{uid}-cadence"
            class="control"
            bind:value={draft.cadence}
            aria-describedby="{uid}-cadence-hint"
          >
            {#each options.cadences as cadence (cadence)}
              <option value={cadence}>{cadenceLabel(cadence)}</option>
            {/each}
          </select>
          <p class="hint" id="{uid}-cadence-hint">
            leaf uses this for reminders. You can change it later in the series settings.
          </p>
        </div>

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
            bind:value={draft.startDay}
            aria-invalid={invalid('startDay')}
            aria-describedby={describedBy('startDay', `${uid}-startDay-hint`)}
            oninput={() => errors.edited('startDay')}
            onkeydown={doneTyping}
          />
          <p class="hint" id="{uid}-startDay-hint">
            Already on Day 200 somewhere else? Start numbering there. Most people leave this at 1.
          </p>
          {@render problem('startDay')}
        </div>
      </div>
    </details>
  </div>

  {#if note}<p class="note">{note}</p>{/if}

  <div class="bar">
    {#if errors.form}<p class="form-error" role="alert">{errors.form}</p>{/if}
    <Button variant="primary" type="submit" full disabled={submitting || noChannels}>
      {submitting ? 'Starting…' : 'Start a series'}
    </Button>
  </div>
</form>

<style>
  .create {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-md);
  }
  /* Whatever is scrolled to (a control taking focus from Tab or the
   * keyboard's next key, a refused field and its message) stops clear of
   * the stuck action bar and of the top inset, not behind them. */
  .create :global(:is(input, select, textarea, button, summary, .field-error)) {
    scroll-margin: calc(var(--safe-top) + var(--space-xl)) 0
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
  .group {
    display: grid;
    gap: var(--space-xxs);
    min-width: 0;
    margin: 0;
    padding: 0;
    border: 0;
  }
  legend {
    padding: 0;
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
    font-weight: var(--fw-emphasis);
  }
  .optional {
    color: var(--ink-subtle);
    font-weight: var(--fw-body);
  }
  .choice {
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    min-height: var(--touch-target);
  }
  .control[aria-invalid='true'] {
    border-color: var(--error-fill);
  }
  .note,
  .field-error,
  .form-error,
  .restored {
    margin: 0;
    font-size: var(--fs-body-sm);
  }
  .hint,
  .count {
    margin: 0;
    font-size: var(--fs-caption);
  }
  .hint,
  .note,
  .restored {
    color: var(--ink-muted);
  }
  .count {
    justify-self: end;
    color: var(--ink-subtle);
    font-variant-numeric: tabular-nums;
  }
  .count.over,
  .field-error,
  .form-error {
    color: var(--error);
  }
  .count.over {
    font-weight: var(--fw-emphasis);
  }
  .restored {
    display: flex;
    flex-wrap: wrap;
    gap: 0 var(--space-xs);
    align-items: center;
  }
  .link {
    min-height: var(--touch-target);
    padding: 0 var(--space-xs);
    color: var(--link);
    font: inherit;
    font-weight: var(--fw-emphasis);
    background: none;
    border: 0;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .hint .link {
    margin-left: calc(-1 * var(--space-xs));
  }

  .more {
    padding-top: var(--space-xs);
    border-top: 1px solid var(--hairline);
  }
  /* Title over the current values, beside a chevron drawn with borders (the
   * bundled fonts have no triangle glyph). */
  .more summary {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    column-gap: var(--space-sm);
    align-items: center;
    min-height: var(--touch-target);
    margin: 0 calc(-1 * var(--space-xs));
    padding: var(--space-xs);
    font-size: var(--fs-body-sm);
    list-style: none;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .more summary::-webkit-details-marker {
    display: none;
  }
  .more summary::before {
    content: '';
    grid-row: span 2;
    width: 8px;
    height: 8px;
    margin: 0 4px 2px 2px;
    border-right: 2px solid var(--link);
    border-bottom: 2px solid var(--link);
    transform: rotate(-45deg);
    transition: transform var(--motion-fast) var(--ease);
  }
  .more[open] summary::before {
    transform: rotate(45deg);
  }
  .more-title {
    color: var(--link);
    font-weight: var(--fw-emphasis);
  }
  .more-values {
    grid-column: 2;
    color: var(--ink-muted);
    font-size: var(--fs-caption);
  }
  .more-body {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-md);
    padding-top: var(--space-xs);
  }

  /* The one action stays in reach: stuck to the bottom of a long form (and
   * above the keyboard where the webview resizes for it), over the bottom
   * inset. A view that pads its sides sets --form-bleed to that padding, so
   * the bar reaches the screen edges and the card scrolls under it. */
  .bar {
    position: sticky;
    bottom: 0;
    z-index: 1;
    display: grid;
    gap: var(--space-xs);
    margin: 0 calc(-1 * var(--form-bleed, 0px)) calc(-1 * var(--safe-bottom));
    padding: var(--space-sm) var(--form-bleed, 0px) max(var(--space-sm), var(--safe-bottom));
    background: var(--canvas);
    box-shadow: 0 -1px 0 var(--hairline);
  }

  @media (hover: hover) {
    .link:hover,
    .more summary:hover .more-title {
      text-decoration: underline;
    }
  }
</style>
