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
</script>

<button class="card" style="--card-accent:{accent}" onclick={() => onSelect(series)}>
  <span class="bar" aria-hidden="true"></span>
  <span class="emoji" aria-hidden="true">{series.emoji}</span>
  <span class="text">
    <span class="name">{series.name}</span>
    {#if series.description}<span class="desc">{series.description}</span>{/if}
  </span>
  <span class="day">{dayLabel}</span>
</button>

<style>
  .card {
    display: grid;
    grid-template-columns: auto auto 1fr auto;
    align-items: center;
    gap: var(--space-sm);
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
  .card:hover {
    background: var(--surface-2);
  }
  .card:focus-visible {
    border-color: var(--accent, var(--card-accent));
    outline: none;
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
    gap: 2px;
    min-width: 0;
  }
  .name {
    font-weight: var(--fw-emphasis);
  }
  .desc {
    overflow: hidden;
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .day {
    padding: 4px 10px;
    color: var(--ink-muted);
    font-size: var(--fs-caption);
    white-space: nowrap;
    background: var(--surface-2);
    border-radius: var(--radius-pill);
  }
</style>
