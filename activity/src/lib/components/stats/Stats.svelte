<script lang="ts">
  import type { Stats } from '../../types/api';
  import { lastPostLabel } from '../../utils/calendar';
  import Skeleton from '../shared/Skeleton.svelte';
  import Card from '../ui/Card.svelte';

  interface Props {
    /** `null` while loading: the card keeps its shape with placeholder values. */
    stats: Stats | null;
    /** Newest post time, unix seconds; adds a "Last post" line. */
    lastPostedAt?: number | null | undefined;
    /** The current time, epoch milliseconds (tests). */
    now?: number | undefined;
  }
  let { stats, lastPostedAt = null, now }: Props = $props();

  function days(n: number): string {
    return n === 1 ? 'day' : 'days';
  }

  // The runs count consecutive day numbers, not dates (a series that stopped
  // posting keeps its last run), hence "Latest run" rather than "streak".
  const items = $derived([
    { label: 'Latest run', value: stats?.current_streak, unit: true, accent: true },
    { label: 'Longest run', value: stats?.longest_streak, unit: true, accent: false },
    { label: 'Days archived', value: stats?.total, unit: false, accent: false },
    { label: 'Skipped day numbers', value: stats?.missed, unit: false, accent: false },
  ]);
  const lastPost = $derived(
    // A Date holds ±8.64e15 ms; anything past that (a bad import) is skipped.
    lastPostedAt !== null && Math.abs(lastPostedAt) <= 8.64e12
      ? {
          label: lastPostLabel(lastPostedAt, now ?? Date.now()),
          iso: new Date(lastPostedAt * 1000).toISOString(),
        }
      : null,
  );
</script>

<Card label="Series statistics">
  <p class="eyebrow">Stats</p>
  <dl class="grid">
    {#each items as item, i (item.label)}
      <div class="stat">
        <dt>{item.label}</dt>
        <dd class:accent={item.accent}>
          {#if item.value === undefined}
            <!-- One labelled placeholder per card; the rest are decorative. -->
            <Skeleton width="3ch" height="1.1em" label={i === 0 ? 'Loading stats' : ''} />
          {:else}
            {item.value}{#if item.unit}<span class="unit">{` ${days(item.value)}`}</span>{/if}
          {/if}
        </dd>
      </div>
    {/each}
  </dl>
  {#if lastPost}
    <p class="last">
      Last post <time datetime={lastPost.iso}>{lastPost.label}</time>
    </p>
  {/if}
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
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--space-sm);
    margin: 0;
  }
  .stat {
    display: grid;
    gap: 2px;
    /* A label that wraps pushes its number down: keep the numbers in a row. */
    align-content: space-between;
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
    /* Darken the pastel per-series accent toward ink so the highlighted
     * stat stays legible as colored text on a white card. */
    color: color-mix(in oklab, var(--accent), var(--ink) 38%);
  }
  .unit {
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
    font-weight: var(--fw-body);
  }
  .last {
    margin: var(--space-sm) 0 0;
    color: var(--ink-muted);
    font-size: var(--fs-caption);
  }
  .last time {
    color: var(--ink);
    font-weight: var(--fw-emphasis);
  }
  /* Below the two-column layout the card sits above the calendar, so it is
   * a compact table (label left, number right) and the calendar keeps the
   * first screen. The card's own label names it for a screen reader. */
  @media (max-width: 959.98px) {
    .eyebrow {
      display: none;
    }
    .grid {
      /* Two columns down to a 320px phone, four in a wider window. */
      grid-template-columns: repeat(auto-fit, minmax(7.5rem, 1fr));
      gap: var(--space-xs) var(--space-sm);
    }
    .stat {
      display: flex;
      gap: var(--space-xs);
      align-items: baseline;
      justify-content: space-between;
    }
    dd {
      flex: none;
      font-size: var(--fs-subhead);
    }
    .last {
      margin-top: var(--space-xs);
    }
  }
  @media (min-width: 960px) {
    .grid {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
