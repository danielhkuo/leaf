<script lang="ts">
  import type { Stats } from '../../types/api';
  import Card from '../ui/Card.svelte';

  interface Props {
    stats: Stats;
  }
  let { stats }: Props = $props();

  const items = $derived([
    { label: 'Current streak', value: stats.current_streak, accent: true },
    { label: 'Longest streak', value: stats.longest_streak, accent: false },
    { label: 'Total days', value: stats.total, accent: false },
    { label: 'Missed', value: stats.missed, accent: false },
  ]);
</script>

<Card label="Series statistics">
  <p class="eyebrow">Stats</p>
  <dl class="grid">
    {#each items as item (item.label)}
      <div class="stat">
        <dt>{item.label}</dt>
        <dd class:accent={item.accent}>{item.value}</dd>
      </div>
    {/each}
  </dl>
</Card>

<style>
  .eyebrow {
    margin: 0 0 var(--space-sm);
    color: var(--ink-subtle);
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    letter-spacing: 0.6px;
    text-transform: uppercase;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: var(--space-sm);
    margin: 0;
  }
  .stat {
    display: grid;
    gap: 2px;
  }
  dt {
    color: var(--ink-muted);
    font-size: var(--fs-caption);
  }
  dd {
    margin: 0;
    font-size: var(--fs-headline);
    font-weight: var(--fw-display);
    line-height: 1.1;
    font-variant-numeric: tabular-nums;
  }
  dd.accent {
    color: var(--accent);
  }
  @media (min-width: 960px) {
    .grid {
      grid-template-columns: 1fr;
    }
  }
</style>
