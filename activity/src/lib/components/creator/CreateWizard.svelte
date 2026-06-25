<script lang="ts">
  // Multi-step "start a series" form. Steps validate locally before
  // advancing; the final submit re-runs server policy (the real gate).
  import type { CreateSeriesInput, SeriesOptions } from '../../types/api';
  import { cadenceLabel, privacyLabel } from '../../utils/labels';
  import Button from '../ui/Button.svelte';

  interface Props {
    options: SeriesOptions;
    submitting: boolean;
    error: string | null;
    onSubmit: (input: CreateSeriesInput) => void;
  }
  let { options, submitting, error, onSubmit }: Props = $props();

  // The wizard is mounted with fixed `options`, so seeding editable state from
  // them once is intentional (not a missed derived).
  let name = $state('');
  let description = $state('');
  // svelte-ignore state_referenced_locally
  let channelId = $state(options.channels[0]?.id ?? '');
  // svelte-ignore state_referenced_locally
  let cadence = $state(options.cadences[0] ?? 'daily');
  // svelte-ignore state_referenced_locally
  let privacy = $state(options.privacy_modes[0] ?? 'public');
  // svelte-ignore state_referenced_locally
  let privacyRoleId = $state(options.roles[0]?.id ?? '');
  let startDay = $state(1);

  const steps = ['Name', 'Channel', 'Cadence', 'Privacy', 'Start day', 'Confirm'] as const;
  let step = $state(0);
  let stepError = $state<string | null>(null);

  const trimmedName = $derived(name.trim());

  function validateStep(i: number): string | null {
    if (i === 0) {
      const len = [...trimmedName].length;
      if (len < 2 || len > 40) return 'Give it a name between 2 and 40 characters.';
      if ([...description].length > 200) return 'Keep the description under 200 characters.';
    }
    if (i === 1 && !channelId) return 'Pick a channel to post in.';
    if (i === 3 && privacy === 'role_gated' && !privacyRoleId) return 'Choose a role to gate on.';
    if (i === 4 && (!Number.isInteger(startDay) || startDay < 1))
      return 'Start day must be 1 or more.';
    return null;
  }

  function next(): void {
    const err = validateStep(step);
    if (err) {
      stepError = err;
      return;
    }
    stepError = null;
    if (step < steps.length - 1) step += 1;
  }

  function back(): void {
    stepError = null;
    if (step > 0) step -= 1;
  }

  function submit(): void {
    onSubmit({
      name: trimmedName,
      description: description.trim(),
      channel_id: channelId,
      cadence,
      privacy,
      privacy_role_id: privacy === 'role_gated' ? privacyRoleId : null,
      start_day: startDay,
    });
  }

  const channelName = $derived(options.channels.find((c) => c.id === channelId)?.name ?? channelId);
  const roleName = $derived(options.roles.find((r) => r.id === privacyRoleId)?.name ?? '');
</script>

<nav class="steps" aria-label="Create series">
  <span class="eyebrow">Step {step + 1} of {steps.length}</span>
  <span class="step-name">{steps[step]}</span>
</nav>

<div class="panel">
  {#if step === 0}
    <label class="field">
      <span class="field-label">Series name</span>
      <input class="control" bind:value={name} maxlength="40" placeholder="Daily sketch" />
    </label>
    <label class="field">
      <span class="field-label">Description <em>(optional)</em></span>
      <textarea class="control" bind:value={description} maxlength="200" rows="3"></textarea>
    </label>
  {:else if step === 1}
    <fieldset class="field">
      <legend class="field-label">Channel</legend>
      {#if options.channels.length === 0}
        <p class="muted">No watched channels — an admin sets these in /setup.</p>
      {:else}
        {#each options.channels as ch (ch.id)}
          <label class="radio">
            <input type="radio" name="channel" value={ch.id} bind:group={channelId} />
            <span>#{ch.name}</span>
          </label>
        {/each}
      {/if}
    </fieldset>
  {:else if step === 2}
    <fieldset class="field">
      <legend class="field-label">How often will you post?</legend>
      {#each options.cadences as c (c)}
        <label class="radio">
          <input type="radio" name="cadence" value={c} bind:group={cadence} />
          <span>{cadenceLabel(c)}</span>
        </label>
      {/each}
    </fieldset>
  {:else if step === 3}
    <fieldset class="field">
      <legend class="field-label">Who can see it?</legend>
      {#each options.privacy_modes as p (p)}
        <label class="radio">
          <input type="radio" name="privacy" value={p} bind:group={privacy} />
          <span>{privacyLabel(p)}</span>
        </label>
      {/each}
    </fieldset>
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
  {:else if step === 4}
    <label class="field">
      <span class="field-label">First day number</span>
      <input class="control" type="number" min="1" bind:value={startDay} />
    </label>
  {:else}
    <dl class="summary">
      <div>
        <dt>Name</dt>
        <dd>{trimmedName}</dd>
      </div>
      <div>
        <dt>Channel</dt>
        <dd>#{channelName}</dd>
      </div>
      <div>
        <dt>Cadence</dt>
        <dd>{cadenceLabel(cadence)}</dd>
      </div>
      <div>
        <dt>Privacy</dt>
        <dd>
          {privacyLabel(privacy)}{#if privacy === 'role_gated' && roleName}
            · {roleName}{/if}
        </dd>
      </div>
      <div>
        <dt>Start day</dt>
        <dd>{startDay}</dd>
      </div>
    </dl>
    {#if options.sprout_enabled}
      <p class="muted">
        New series start as a 🌱 sprout: archive {options.sprout_threshold} posts and it goes public.
      </p>
    {/if}
  {/if}

  {#if stepError}<p class="inline-error" role="alert">{stepError}</p>{/if}
  {#if error}<p class="inline-error" role="alert">{error}</p>{/if}
</div>

<div class="actions">
  {#if step > 0}
    <Button variant="ghost" onclick={back}>Back</Button>
  {/if}
  {#if step < steps.length - 1}
    <Button variant="primary" onclick={next}>Next</Button>
  {:else}
    <Button variant="primary" disabled={submitting} onclick={submit}>
      {submitting ? 'Planting…' : 'Start series'}
    </Button>
  {/if}
</div>

<style>
  .steps {
    display: flex;
    gap: var(--space-sm);
    align-items: baseline;
    margin-bottom: var(--space-sm);
  }
  .eyebrow {
    color: var(--ink-subtle);
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    letter-spacing: 0.6px;
    text-transform: uppercase;
  }
  .step-name {
    font-size: var(--fs-subhead);
    font-weight: var(--fw-display);
  }
  .panel {
    display: grid;
    gap: var(--space-md);
    padding: var(--space-md);
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-xl);
    box-shadow: var(--shadow-card);
  }
  fieldset.field {
    margin: 0;
    padding: 0;
    border: 0;
  }
  legend {
    padding: 0;
  }
  .field em {
    color: var(--ink-subtle);
    font-style: normal;
  }
  .radio {
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    min-height: var(--touch-target);
  }
  .muted {
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }
  .summary {
    display: grid;
    gap: var(--space-xs);
    margin: 0;
  }
  .summary div {
    display: grid;
    grid-template-columns: 7rem 1fr;
    gap: var(--space-sm);
  }
  .summary dt {
    color: var(--ink-subtle);
    font-size: var(--fs-body-sm);
  }
  .summary dd {
    margin: 0;
  }
  .inline-error {
    margin: 0;
    color: var(--error);
    font-size: var(--fs-body-sm);
  }
  .actions {
    display: flex;
    gap: var(--space-sm);
    justify-content: flex-end;
    margin-top: var(--space-md);
  }
</style>
