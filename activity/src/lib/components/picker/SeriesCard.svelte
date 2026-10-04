<script lang="ts">
  import type { Series } from '../../types/api';
  import { accentVar } from '../../utils/accent';

  interface Props {
    series: Series;
    onSelect: (series: Series) => void;
  }
  let { series, onSelect }: Props = $props();

  const accent = $derived(accentVar(series.id));
  const dayLabel = $derived(series.max_day !== null ? `Day ${series.max_day}` : 'No days yet');
  // A revoked series is only ever listed for its creator, and its days no
  // longer load: it is shown, with the reason, but cannot be opened.
  const revoked = $derived(series.state === 'revoked');
  /**
   * What only the owner needs to know: who else can see the series. A sprout
   * is hidden from others until the threshold, then its privacy decides; for
   * "Only me" that means nobody else, before or after.
   */
  const status = $derived.by(() => {
    const { sprout, privacy } = series;
    if (sprout && privacy === 'creator_only') {
      return `🌱 Only you can see this series. Its privacy is Only me, so that stays the same after ${sprout.threshold} days.`;
    }
    if (sprout) {
      const until = `🌱 Only you can see this until ${sprout.threshold} days are archived (${sprout.archived} so far).`;
      if (privacy === 'public') return `${until} Then everyone in the server can.`;
      if (privacy === 'role_gated') return `${until} Then members with its role can.`;
      return `${until} Then its privacy setting applies.`;
    }
    if (series.is_owner && privacy === 'creator_only') return 'Only you can see this series.';
    if (series.is_owner && privacy === 'role_gated')
      return 'Only members with its role can see it.';
    return null;
  });
</script>

{#if revoked}
  <div class="card revoked" style="--card-accent:{accent}">
    <span class="bar" aria-hidden="true"></span>
    <span class="emoji" aria-hidden="true">{series.emoji}</span>
    <span class="text">
      <span class="name">{series.name}</span>
    </span>
    <span class="day">Revoked</span>
    <span class="status">
      A server admin revoked this series. It’s hidden from everyone and can’t take new posts; its
      days are kept. Ask an admin to restore it.
    </span>
  </div>
{:else}
  <button
    type="button"
    class="card"
    class:with-status={status !== null}
    style="--card-accent:{accent}"
    onclick={() => onSelect(series)}
  >
    <span class="bar" aria-hidden="true"></span>
    <span class="emoji" aria-hidden="true">{series.emoji}</span>
    <span class="text">
      <span class="name">{series.name}</span>
      {#if series.description}<span class="desc">{series.description}</span>{/if}
    </span>
    <span class="day">{dayLabel}</span>
    {#if status}<span class="status">{status}</span>{/if}
  </button>
{/if}

<style>
  .card {
    display: grid;
    grid-template-columns: auto auto minmax(0, 1fr) auto;
    align-items: center;
    gap: var(--space-xxs) var(--space-sm);
    width: 100%;
    min-height: var(--control-height);
    padding: var(--space-md);
    text-align: left;
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-xl);
    box-shadow: var(--shadow-card);
    color: var(--ink);
    font: inherit;
    cursor: pointer;
    transition:
      background var(--motion-fast) var(--ease),
      border-color var(--motion-fast) var(--ease);
  }
  button.card:active {
    background: var(--surface-3);
  }
  .revoked {
    background: var(--surface-2);
    box-shadow: none;
    cursor: default;
  }
  .revoked .name {
    color: var(--ink-muted);
  }
  .bar {
    align-self: stretch;
    width: 4px;
    background: var(--card-accent);
    border-radius: var(--radius-pill);
  }
  /* The accent bar runs down beside the status line too. */
  .revoked .bar,
  .with-status .bar {
    grid-row: span 2;
  }
  .revoked .bar {
    background: var(--hairline-strong);
  }
  .emoji {
    font-size: 1.5rem;
  }
  .text {
    display: grid;
    gap: 2px;
    min-width: 0;
  }
  .name {
    overflow-wrap: anywhere;
    font-weight: var(--fw-emphasis);
  }
  .desc {
    overflow: hidden;
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* Under the name and the day pill, using the card's full text width. */
  .status {
    grid-column: 3 / -1;
    color: var(--ink-muted);
    font-size: var(--fs-caption);
  }
  .day {
    padding: 4px 10px;
    color: var(--ink-muted);
    font-size: var(--fs-caption);
    white-space: nowrap;
    background: var(--surface-2);
    border-radius: var(--radius-pill);
  }
  .revoked .day {
    background: var(--surface-3);
  }
  @media (hover: hover) {
    button.card:hover {
      background: var(--surface-2);
    }
  }
</style>
