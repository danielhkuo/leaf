<script lang="ts">
  import { untrack } from 'svelte';

  import DayViewer from '../lib/components/viewer/DayViewer.svelte';
  import { gallery, getDay, loadDaysIndex, peekDay } from '../lib/stores/gallery.svelte';
  import type { Day, DaySummary, Series } from '../lib/types/api';
  import { accentVar } from '../lib/utils/accent';
  import { describeError, errorKind, isRetryable } from '../lib/utils/errors';
  import { nextDay, prevDay, randomDay } from '../lib/utils/navigation';

  interface Props {
    series: Series;
    day: number;
    /** Called with the day on screen, so Home can bring its cell into view. */
    onClose: (lastDay?: number) => void;
  }
  let { series, day, onClose }: Props = $props();

  // The viewer remounts per open, so the initial day is captured intentionally;
  // prev/next then drive `currentDay` locally.
  let currentDay = $state(untrack(() => day));
  let index = $state<DaySummary[]>([]);

  // The last day that loaded and the last load that failed. Each counts only
  // while it is for the day on screen, so stepping to another day shows that
  // day's loading state at once, inside the same shell.
  let loaded = $state<Day | null>(untrack(() => peekDay(series.id, day)));
  let failure = $state<{ day: number; message: string; retry: boolean } | null>(null);
  const dayData = $derived(loaded?.day === currentDay ? loaded : null);
  const failed = $derived(failure?.day === currentDay ? failure : null);

  let reloads = $state(0);
  /** The next load skips the cache: a Retry, or a photo whose address expired. */
  let fresh = false;

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
  const byDay = $derived(new Map(index.map((r) => [r.day, r])));
  const prev = $derived(prevDay(days, currentDay));
  const next = $derived(nextDay(days, currentDay));

  // The present-day index (gap-aware nav, previews). Loaded again after a
  // refresh of the gallery; the old one stays in use until the new one is in.
  $effect(() => {
    void gallery.epoch;
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

  // Full-res for the open day only. Days already seen come from the store's
  // cache with no loading state. After a refresh of the gallery the day on
  // screen stays until its new data arrives.
  $effect(() => {
    const target = currentDay;
    const seriesId = series.id;
    void gallery.epoch;
    void reloads;
    const skipCache = fresh;
    fresh = false;

    if (!skipCache) {
      const cached = peekDay(seriesId, target);
      if (cached) {
        loaded = cached;
        failure = null;
        return;
      }
    }

    let cancelled = false;
    getDay(seriesId, target, { fresh: skipCache })
      .then((data) => {
        if (cancelled) return;
        loaded = data;
        failure = null;
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        console.error(`leaf: loading Day ${target} failed`, e);
        const gone = errorKind(e) === 'not_found';
        // A reload that failed for a passing reason leaves the day on screen.
        if (!gone && untrack(() => loaded?.day === target)) return;
        loaded = null;
        failure = {
          day: target,
          message: gone
            ? 'This day isn’t in the archive. It may have been removed.'
            : describeError(e),
          retry: isRetryable(e),
        };
      });
    return () => {
      cancelled = true;
    };
  });

  // Preload only the two adjacent thumbnails — never adjacent full-res.
  $effect(() => {
    for (const d of [prev, next]) {
      if (d === null) continue;
      const thumb = byDay.get(d)?.thumb_url;
      if (thumb) {
        const img = new Image();
        img.src = thumb;
      }
    }
  });

  function reload(): void {
    fresh = true;
    reloads += 1;
  }
  function goRandom(): void {
    const r = randomDay(days, currentDay);
    if (r !== null) currentDay = r;
  }
</script>

<div class="overlay" style="--accent:{accent}">
  <DayViewer
    dayNumber={currentDay}
    day={dayData}
    error={failed?.message}
    summary={byDay.get(currentDay) ?? null}
    seriesName={series.name}
    timeZone={series.timezone}
    hasPrev={prev !== null}
    hasNext={next !== null}
    onPrev={() => prev !== null && (currentDay = prev)}
    onNext={() => next !== null && (currentDay = next)}
    onRandom={days.some((d) => d !== currentDay) ? goRandom : undefined}
    onClose={() => onClose(currentDay)}
    onRetry={dayData || failed?.retry ? reload : undefined}
  />
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 10;
    background: var(--canvas);
    overscroll-behavior: contain;
  }
</style>
