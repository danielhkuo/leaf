<script lang="ts">
  import Calendar from '../lib/components/calendar/Calendar.svelte';
  import Callout from '../lib/components/shared/Callout.svelte';
  import Skeleton from '../lib/components/shared/Skeleton.svelte';
  import StatsPanel from '../lib/components/stats/Stats.svelte';
  import IconButton from '../lib/components/ui/IconButton.svelte';
  import { getApi, getGuildId, loadDaysIndex } from '../lib/stores/gallery.svelte';
  import { nav } from '../lib/stores/nav.svelte';
  import type { DaySummary, Series, Stats } from '../lib/types/api';
  import { accentVar } from '../lib/utils/accent';

  interface Props {
    series: Series;
    userId: string;
    canGoBack: boolean;
  }
  let { series, userId, canGoBack }: Props = $props();

  const accent = $derived(accentVar(series.id));
  const maxDay = $derived(series.max_day ?? 0);
  const isOwner = $derived(series.creator_id === userId);

  let stats = $state<Stats | null>(null);
  let statsFailed = $state(false);
  let index = $state<DaySummary[] | null>(null);
  let indexFailed = $state(false);

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

  // The full present-day index (day + posted_at + thumb), cached per series;
  // feeds the calendar. Also primed for the viewer's gap-aware navigation.
  $effect(() => {
    const id = series.id;
    const md = maxDay;
    let cancelled = false;
    index = null;
    indexFailed = false;
    loadDaysIndex(id, md)
      .then((rows) => {
        if (!cancelled) index = rows;
      })
      .catch(() => {
        if (!cancelled) indexFailed = true;
      });
    return () => {
      cancelled = true;
    };
  });

  function openDay(day: number): void {
    nav.push({ name: 'viewer', seriesId: series.id, day });
  }
</script>

<div class="home" style="--accent:{accent}">
  <header class="bar">
    {#if canGoBack}
      <IconButton ariaLabel="Back to series list" variant="solid" onclick={() => nav.back()}>
        ←
      </IconButton>
    {/if}
    <span class="emoji" aria-hidden="true">{series.emoji}</span>
    <h1>{series.name}</h1>
    {#if isOwner}
      <span class="spacer"></span>
      <IconButton
        ariaLabel="Series settings"
        variant="solid"
        onclick={() => nav.push({ name: 'seriesSettings', seriesId: series.id })}
      >
        ⚙
      </IconButton>
    {/if}
  </header>

  {#if maxDay === 0}
    <Callout title="Still sprouting">
      No days archived yet — they’ll appear here as they’re posted.
    </Callout>
  {:else}
    <div class="content">
      <aside class="side">
        {#if stats}
          <StatsPanel {stats} />
        {:else if statsFailed}
          <p class="muted">Stats unavailable.</p>
        {:else}
          <Skeleton height="132px" radius="var(--radius-xl)" />
        {/if}
      </aside>
      <main class="main">
        {#if index}
          <Calendar {index} onOpenDay={openDay} />
        {:else if indexFailed}
          <Callout title="Couldn’t load the calendar">Try reopening the gallery.</Callout>
        {:else}
          <Skeleton height="340px" radius="var(--radius-xl)" />
        {/if}
      </main>
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
  .emoji {
    font-size: 1.5rem;
  }
  .spacer {
    flex: 1;
  }
  h1 {
    margin: 0;
    font-size: var(--fs-card-title);
    font-weight: var(--fw-display);
    letter-spacing: var(--tracking-display);
  }
  .content {
    display: grid;
    gap: var(--space-lg);
    grid-template-areas: 'side' 'main';
  }
  .side {
    grid-area: side;
  }
  .main {
    grid-area: main;
    min-width: 0; /* let the calendar grid shrink instead of overflowing */
  }
  .muted {
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }
  /* Desktop (DESIGN.md --bp-md 960px): aligned two columns, sticky sidebar. */
  @media (min-width: 960px) {
    .content {
      grid-template-columns: 1fr 16rem;
      grid-template-areas: 'main side';
      align-items: start;
    }
    .side {
      position: sticky;
      top: calc(var(--appbar-h) + var(--space-md));
    }
  }
</style>
