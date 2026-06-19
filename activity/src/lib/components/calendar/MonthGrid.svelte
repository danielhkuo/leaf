<script lang="ts">
  import type { CalendarMonth } from '../../utils/calendar';
  import DayCell from './DayCell.svelte';

  interface Props {
    month: CalendarMonth;
    weekdays: string[];
    onOpenDay: (day: number) => void;
  }
  let { month, weekdays, onOpenDay }: Props = $props();

  // Blank cells before the 1st, as a plain index list (keyed, nothing unused).
  const pads = $derived([...Array(month.leading).keys()]);
</script>

<!-- `content-visibility` lets the browser skip layout/paint for off-screen
     months while `contain-intrinsic-size` reserves their space, so the scroll
     position stays stable across a multi-year archive. -->
<section class="month">
  <h2 class="label">{month.label}</h2>
  <div class="weekdays" aria-hidden="true">
    {#each weekdays as w, i (i)}
      <span>{w}</span>
    {/each}
  </div>
  <div class="grid">
    {#each pads as i (i)}
      <span class="pad" aria-hidden="true"></span>
    {/each}
    {#each month.cells as cell (cell.date)}
      <DayCell {cell} {onOpenDay} />
    {/each}
  </div>
</section>

<style>
  .month {
    content-visibility: auto;
    contain-intrinsic-size: auto 560px;
  }
  .label {
    margin: 0 0 var(--space-xs);
    font-size: var(--fs-subhead);
    font-weight: var(--fw-display);
    letter-spacing: var(--tracking-display);
  }
  .weekdays,
  .grid {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: var(--space-xxs);
  }
  .weekdays {
    margin-bottom: var(--space-xxs);
  }
  .weekdays span {
    padding-bottom: 2px;
    color: var(--ink-subtle);
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    text-align: center;
  }
</style>
