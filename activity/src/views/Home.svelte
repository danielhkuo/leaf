<script lang="ts">
  import Heatmap from '../lib/components/heatmap/Heatmap.svelte';
  import Callout from '../lib/components/shared/Callout.svelte';
  import Skeleton from '../lib/components/shared/Skeleton.svelte';
  import StatsPanel from '../lib/components/stats/Stats.svelte';
  import { getApi, getGuildId } from '../lib/stores/gallery.svelte';
  import { nav } from '../lib/stores/nav.svelte';
  import type { DaySummary, Series, Stats } from '../lib/types/api';
  import { accentVar } from '../lib/utils/accent';

  interface Props {
    series: Series;
    canGoBack: boolean;
  }
  let { series, canGoBack }: Props = $props();

  const accent = $derived(accentVar(series.id));
  const maxDay = $derived(series.max_day ?? 0);

  let stats = $state<Stats | null>(null);
  let statsFailed = $state(false);

  $effect(() => {
    const id = series.id;
    let cancelled = false;
    stats = null;
    statsFailed = false;
    getApi()
      .getStats(getGuildId(), id)
      .then((s) => {
        if (!cancelled) stats = s;
      })
      .catch(() => {
        if (!cancelled) statsFailed = true;
      });
    return () => {
      cancelled = true;
    };
  });

  function loadDays(from: number, to: number): Promise<DaySummary[]> {
    return getApi().listDays(getGuildId(), series.id, { from, to });
  }
  function openDay(day: number): void {
    nav.push({ name: 'viewer', seriesId: series.id, day });
  }
</script>

<div class="home" style="--accent:{accent}">
  <header class="bar">
    {#if canGoBack}
      <button class="back" onclick={() => nav.back()} aria-label="Back to series list">←</button>
    {/if}
    <span class="emoji" aria-hidden="true">{series.emoji}</span>
    <h1>{series.name}</h1>
  </header>

  {#if maxDay === 0}
    <Callout title="Still sprouting">
      No days archived yet — they’ll appear here as they’re posted.
    </Callout>
  {:else}
    <div class="content">
      <div class="grid-col">
        <Heatmap {maxDay} startDay={series.start_day} load={loadDays} onOpenDay={openDay} />
      </div>
      <aside class="side">
        {#if stats}
          <StatsPanel {stats} />
        {:else if statsFailed}
          <p class="muted">Stats unavailable.</p>
        {:else}
          <Skeleton height="180px" radius="var(--radius-lg)" />
        {/if}
      </aside>
    </div>
  {/if}
</div>

<style>
  .home {
    display: grid;
    gap: var(--space-md);
    width: 100%;
    max-width: 64rem;
    margin: 0 auto;
    padding: var(--space-md);
  }
  .bar {
    position: sticky;
    top: 0;
    z-index: 1;
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    min-height: var(--appbar-h);
    background: var(--canvas);
  }
  .back {
    width: 36px;
    height: 36px;
    color: var(--ink);
    font: inherit;
    font-size: 1.25rem;
    background: var(--surface-1);
    border: 1px solid var(--hairline);
    border-radius: var(--radius-md);
    cursor: pointer;
  }
  .emoji {
    font-size: 1.5rem;
  }
  h1 {
    margin: 0;
    font-size: var(--fs-card-title);
    font-weight: var(--fw-emphasis);
  }
  .content {
    display: grid;
    gap: var(--space-lg);
  }
  .muted {
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }
  @media (min-width: 1024px) {
    .content {
      grid-template-columns: 1fr 16rem;
      align-items: start;
    }
    .side {
      position: sticky;
      top: var(--appbar-h);
    }
  }
</style>
