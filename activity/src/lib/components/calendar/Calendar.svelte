<script lang="ts">
  import type { DaySummary } from '../../types/api';
  import {
    buildMonths,
    weekStartFor,
    weekdayLabels,
    type CalendarLayout,
    type MonthCell,
  } from '../../utils/calendar';
  import DayCell from './DayCell.svelte';
  import MonthGrid from './MonthGrid.svelte';

  interface Props {
    /** The full archived-day index for the series. */
    index: DaySummary[];
    onOpenDay: (day: number) => void;
    /**
     * The index already laid out with {@link buildMonths} (Home builds it
     * once for its month picker too). Built from `index` when absent.
     */
    layout?: CalendarLayout | null | undefined;
    /** The zone the server dated the days in; shown once above the months. */
    timeZone?: string | undefined;
    /** First day of the week, 0 = Sunday; the device locale's by default. */
    weekStart?: number | undefined;
  }
  let { index, onOpenDay, layout, timeZone, weekStart = weekStartFor() }: Props = $props();

  const built = $derived(layout ?? buildMonths(index, { timeZone, weekStart }));
  // Newest month first: the latest post is what people open the gallery for,
  // and scrolling down goes back in time.
  const items = $derived([...built.items].reverse());
  // Single-letter columns, Apple-Calendar style (S M T W T F S).
  const weekdays = $derived(weekdayLabels(undefined, 'narrow', weekStart));
  const undatedCells = $derived(
    built.undated.map(
      (entry): MonthCell => ({
        date: null,
        entries: [entry],
        kind: 'archived',
        today: false,
        dateLabel: '',
      }),
    ),
  );
</script>

<div class="calendar">
  {#if timeZone && items.length > 0}
    <p class="zone">Dates are in {timeZone} time.</p>
  {/if}
  {#each items as item (item.key)}
    {#if item.kind === 'month'}
      <MonthGrid month={item} {weekdays} {onOpenDay} />
    {:else}
      <p class="gap">{item.label}</p>
    {/if}
  {/each}
  {#if undatedCells.length > 0}
    <section class="undated">
      <h2 class="label">Undated</h2>
      <p class="note">These days have a post time leaf can’t place on the calendar.</p>
      <div class="grid">
        {#each undatedCells as cell (cell.entries[0]?.day)}
          <DayCell {cell} {onOpenDay} />
        {/each}
      </div>
    </section>
  {/if}
</div>

<style>
  .calendar {
    /* Lets each month estimate its height from the calendar's width. */
    container-type: inline-size;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--space-xl);
  }
  .zone,
  .note {
    margin: 0;
    color: var(--ink-muted);
    font-size: var(--fs-caption);
  }
  .zone {
    margin-bottom: calc(var(--space-md) - var(--space-xl));
  }
  .gap {
    margin: 0;
    padding: var(--space-sm) var(--space-md);
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
    text-align: center;
    border: 1px dashed var(--hairline-strong);
    border-radius: var(--radius-lg);
  }
  .label {
    margin: 0 0 var(--space-xxs);
    font-size: var(--fs-subhead);
    font-weight: var(--fw-display);
    letter-spacing: var(--tracking-display);
  }
  .undated .note {
    margin-bottom: var(--space-xs);
  }
  .undated .grid {
    display: grid;
    grid-template-columns: repeat(7, minmax(0, 1fr));
    gap: var(--space-xxs);
  }
  /* As MonthGrid: thinner gaps on the narrowest phones. */
  @media (max-width: 344px) {
    .undated .grid {
      gap: 2px;
    }
  }
</style>
