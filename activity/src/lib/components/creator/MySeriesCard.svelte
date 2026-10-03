<script lang="ts">
  // One of the creator's own series in the "My series" list: identity, what
  // state it is in and what that means, and a quick read of cadence, channel,
  // archived days and reminders. Opens its settings.
  import type { MySeries, SproutProgress } from '../../types/api';
  import { accentVar } from '../../utils/accent';
  import Icon from '../ui/Icon.svelte';
  import { daysAre } from './formRules';

  interface Props {
    series: MySeries;
    onOpen: (series: MySeries) => void;
    /** The sprout threshold, from the gallery's own list when it has one. */
    sprout?: SproutProgress | undefined;
    /** `public` / `role_gated` / `creator_only`, when known. */
    privacy?: string | undefined;
  }
  let { series, onOpen, sprout, privacy }: Props = $props();

  const accent = $derived(accentVar(series.id));
  const revoked = $derived(series.state === 'revoked');
  const isSprout = $derived(series.state === 'sprout');

  const badge = $derived.by(() => {
    if (revoked) return 'Revoked';
    if (!isSprout) return null;
    return sprout ? `🌱 Sprout · ${series.archived_days} of ${sprout.threshold} days` : '🌱 Sprout';
  });

  /** What the state means for who can see the series, in one line. */
  const status = $derived.by(() => {
    if (revoked) {
      return 'A server admin revoked this series. It’s hidden from everyone and can’t be changed or take new posts; its days are kept. Ask an admin to restore it.';
    }
    if (!isSprout) return null;
    if (privacy === 'creator_only') return 'Only you can see it. Its privacy is Only me.';
    const until = sprout
      ? `Only you can see it until ${daysAre(sprout.threshold)} archived.`
      : 'Only you can see it until it has enough archived days.';
    if (privacy === 'public') return `${until} Then everyone in the server can.`;
    if (privacy === 'role_gated') return `${until} Then members with its role can.`;
    return `${until} Then its privacy setting applies.`;
  });

  const archived = $derived(
    series.archived_days === 1 ? '1 day archived' : `${series.archived_days} days archived`,
  );
  /** A reminder that is on but could not be delivered is not "on" to the creator. */
  const undelivered = $derived(series.reminder_enabled && series.reminder_error !== undefined);
</script>

<!-- Only spans inside: a button may not hold block content. -->
<button type="button" class="card" style="--card-accent:{accent}" onclick={() => onOpen(series)}>
  <span class="bar" aria-hidden="true"></span>
  <span class="emoji" aria-hidden="true">{series.emoji}</span>
  <span class="text">
    <span class="name">
      <span>{series.name}<span class="sr-only">, settings.</span></span>
      {#if badge}<span class="badge">{badge}</span>{/if}
    </span>
    <span class="sub">
      {series.cadence}
      {#if series.channel_name}· #{series.channel_name}{/if}
      · {archived}
      {#if undelivered}
        · <span class="warn">reminders can’t reach you</span>
      {:else if series.reminder_enabled}
        · reminders on
      {/if}
    </span>
    {#if status}<span class="status">{status}</span>{/if}
  </span>
  <span class="gear" aria-hidden="true"><Icon name="gear" /></span>
</button>

<style>
  .card {
    position: relative;
    display: grid;
    grid-template-columns: auto auto minmax(0, 1fr) auto;
    gap: var(--space-sm);
    align-items: center;
    width: 100%;
    min-height: var(--control-height);
    padding: var(--space-md);
    color: var(--ink);
    font: inherit;
    text-align: left;
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-xl);
    box-shadow: var(--shadow-card);
    cursor: pointer;
    transition: background var(--motion-fast) var(--ease);
  }
  .card:active {
    background: var(--surface-3);
  }
  .bar {
    align-self: stretch;
    width: 4px;
    background: var(--card-accent);
    border-radius: var(--radius-pill);
  }
  .emoji {
    font-size: 1.5rem;
  }
  .text {
    display: grid;
    /* Not `auto`: a long channel or series name would widen the card. */
    grid-template-columns: minmax(0, 1fr);
    gap: 2px;
    min-width: 0;
  }
  .name {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-xxs) var(--space-xs);
    align-items: center;
    overflow-wrap: anywhere;
    font-weight: var(--fw-emphasis);
  }
  .badge {
    padding: 1px 8px;
    color: var(--ink-muted);
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-body);
    white-space: nowrap;
    background: var(--surface-2);
    border-radius: var(--radius-pill);
  }
  .sub {
    color: var(--ink-subtle);
    font-size: var(--fs-caption);
  }
  .warn {
    color: var(--warning);
    font-weight: var(--fw-emphasis);
  }
  .status {
    margin-top: var(--space-xxs);
    color: var(--ink-muted);
    font-size: var(--fs-caption);
  }
  .gear {
    color: var(--ink-subtle);
    font-size: 1.25rem;
  }
  @media (hover: hover) {
    .card:hover {
      background: var(--surface-2);
    }
  }
</style>
