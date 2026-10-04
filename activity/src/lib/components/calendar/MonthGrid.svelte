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

<!-- `content-visibility` lets the browser skip layout and paint for months
     off screen, while `contain-intrinsic-block-size` reserves roughly their
     height (from the row count) so the scrollbar and month jumps stay close.
     Height only: an intrinsic width would size skipped months by it and push
     the page wider than the phone. Empty dates are hidden from screen
     readers; each archived day's button names its full date. -->
<section class="month" data-month={month.key} style="--rows:{month.rows}">
  <!-- Focusable by script: a month jump moves focus here, so Tab goes on
       into that month's days. -->
  <h2 class="label" tabindex="-1">{month.label}</h2>
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
    /* Fallback for webviews without container units: a phone-sized month. */
    contain-intrinsic-block-size: auto 360px;
    /* Heading + weekday row, then square cells a seventh of the width
     * (100cqw is the calendar's width) with 4px gaps between rows. */
    contain-intrinsic-block-size: auto
      calc(62px + var(--rows) * ((100cqw - 24px) / 7) + (var(--rows) - 1) * 4px);
    /* A jump lands the heading below the sticky bar, not under it. */
    scroll-margin-top: calc(var(--appbar-h) + var(--safe-top) + var(--space-sm));
  }
  .label {
    margin: 0 0 var(--space-xs);
    font-size: var(--fs-subhead);
    font-weight: var(--fw-display);
    letter-spacing: var(--tracking-display);
  }
  .label:focus {
    outline: none;
  }
  .weekdays,
  .grid {
    display: grid;
    /* minmax(0, …): a column never grows past a seventh to fit its content. */
    grid-template-columns: repeat(7, minmax(0, 1fr));
    gap: var(--space-xxs);
  }
  .weekdays {
    margin-bottom: var(--space-xxs);
  }
  /* The narrowest phones: thinner gaps keep each day as near 44px as seven
   * columns allow (Home narrows its side padding at the same width). */
  @media (max-width: 344px) {
    .weekdays,
    .grid {
      gap: 2px;
    }
  }
  .weekdays span {
    padding-bottom: 2px;
    color: var(--ink-subtle);
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    text-align: center;
  }
</style>
