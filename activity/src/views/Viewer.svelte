<script lang="ts">
  import { untrack } from 'svelte';

  import DayViewer from '../lib/components/viewer/DayViewer.svelte';
  import Callout from '../lib/components/shared/Callout.svelte';
  import Skeleton from '../lib/components/shared/Skeleton.svelte';
  import Button from '../lib/components/ui/Button.svelte';
  import { openExternalLink } from '../lib/sdk/actions';
  import { getApi, getGuildId, loadDaysIndex } from '../lib/stores/gallery.svelte';
  import type { Day, DaySummary, Series } from '../lib/types/api';
  import { accentVar } from '../lib/utils/accent';
  import { nextDay, prevDay, randomDay } from '../lib/utils/navigation';

  interface Props {
    series: Series;
    day: number;
    onClose: () => void;
  }
  let { series, day, onClose }: Props = $props();

  // The viewer remounts per open, so the initial day is captured intentionally;
  // prev/next then drive `currentDay` locally.
  let currentDay = $state(untrack(() => day));
  let dayData = $state<Day | null>(null);
  let failed = $state(false);
  let index = $state<DaySummary[]>([]);

  // Lock background scrolling for as long as the viewer is open; restore on
  // close. The overlay is fixed, but the page behind it would otherwise still
  // scroll under touch/wheel.
  $effect(() => {
    const previous = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => {
      document.body.style.overflow = previous;
    };
  });

  const accent = $derived(accentVar(series.id));
  const days = $derived(index.map((r) => r.day));
  const thumbByDay = $derived(new Map(index.map((r) => [r.day, r.thumb_url])));
  const prev = $derived(prevDay(days, currentDay));
  const next = $derived(nextDay(days, currentDay));

  // Load the present-day index once (gap-aware nav + adjacent preload).
  $effect(() => {
    let cancelled = false;
    loadDaysIndex(series.id, series.max_day ?? 0)
      .then((rows) => {
        if (!cancelled) index = rows;
      })
      .catch(() => {
        /* navigation just stays single-day */
      });
    return () => {
      cancelled = true;
    };
  });

  // Full-res for the open day only; the previous src is dropped on change.
  $effect(() => {
    const target = currentDay;
    let cancelled = false;
    dayData = null;
    failed = false;
    getApi()
      .getDay(getGuildId(), series.id, target)
      .then((data) => {
        if (!cancelled) dayData = data;
      })
      .catch(() => {
        if (!cancelled) failed = true;
      });
    return () => {
      cancelled = true;
    };
  });

  // Preload only the two adjacent thumbnails — never adjacent full-res.
  $effect(() => {
    for (const d of [prev, next]) {
      if (d === null) continue;
      const thumb = thumbByDay.get(d);
      if (thumb) {
        const img = new Image();
        img.src = thumb;
      }
    }
  });

  function jump(): void {
    if (dayData) void openExternalLink(dayData.jump_url);
  }
  function goRandom(): void {
    const r = randomDay(days, currentDay);
    if (r !== null) currentDay = r;
  }
</script>

<div class="overlay" style="--accent:{accent}">
  {#if dayData}
    <DayViewer
      day={dayData}
      seriesName={series.name}
      hasPrev={prev !== null}
      hasNext={next !== null}
      onPrev={() => prev !== null && (currentDay = prev)}
      onNext={() => next !== null && (currentDay = next)}
      onRandom={goRandom}
      {onClose}
      onJump={jump}
    />
  {:else if failed}
    <div class="state">
      <Callout title="Couldn’t load this day">It may have been removed.</Callout>
      <Button variant="primary" onclick={onClose}>Close</Button>
    </div>
  {:else}
    <div class="state"><Skeleton width="200px" height="16px" /></div>
  {/if}
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 10;
    background: var(--canvas);
    overscroll-behavior: contain;
  }
  .state {
    position: absolute;
    inset: 0;
    display: grid;
    gap: var(--space-md);
    place-content: center;
    justify-items: center;
    padding: var(--space-lg);
  }
</style>
