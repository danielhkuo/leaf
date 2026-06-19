<script lang="ts">
  import type { DaySummary } from '../../types/api';
  import { buildMonths, weekdayLabels } from '../../utils/calendar';
  import MonthGrid from './MonthGrid.svelte';

  interface Props {
    /** The full present-day index for the series (day + posted_at + thumb). */
    index: DaySummary[];
    onOpenDay: (day: number) => void;
  }
  let { index, onOpenDay }: Props = $props();

  const months = $derived(buildMonths(index));
  // Single-letter columns, Apple-Calendar style (S M T W T F S).
  const weekdays = weekdayLabels(undefined, 'narrow');
</script>

<div class="calendar">
  {#each months as month (`${month.year}-${month.month}`)}
    <MonthGrid {month} {weekdays} {onOpenDay} />
  {/each}
</div>

<style>
  .calendar {
    display: grid;
    gap: var(--space-xl);
  }
</style>
