<script lang="ts">
  import { cellLabel, type MonthCell } from '../../utils/calendar';

  interface Props {
    cell: MonthCell;
    onOpenDay: (day: number) => void;
  }
  let { cell, onOpenDay }: Props = $props();

  const first = $derived(cell.entries[0]);
  const more = $derived(cell.entries.length - 1);
  // A thumbnail that failed to load (deleted object, dropped connection)
  // falls back to the hatched "no picture" tile instead of a blank square.
  // Keyed by URL so a refresh that signs a new one gets another try.
  let failedUrl = $state<string | null>(null);
  const thumb = $derived(first?.thumbUrl && first.thumbUrl !== failedUrl ? first.thumbUrl : null);
  // The label follows what is drawn: a failed picture is "no preview" too.
  const label = $derived(cellLabel(cell, Boolean(first?.thumbUrl) && thumb === null));
</script>

{#if first}
  <button
    type="button"
    class="cell archived"
    class:today={cell.today}
    data-days={cell.entries.map((e) => e.day).join(' ')}
    aria-label={label}
    onclick={() => onOpenDay(first.day)}
  >
    {#if thumb}
      <img
        src={thumb}
        alt=""
        loading="lazy"
        decoding="async"
        width="120"
        height="120"
        onerror={() => (failedUrl = thumb)}
      />
    {:else}
      <span class="missing"></span>
    {/if}
    {#if cell.date !== null}<span class="date">{cell.date}</span>{/if}
    {#if more > 0}<span class="more">+{more}</span>{/if}
    <span class="daynum">{first.day}</span>
  </button>
{:else}
  <span class="cell {cell.kind}" class:today={cell.today} aria-hidden="true">
    <span class="date">{cell.date}</span>
  </span>
{/if}

<style>
  .cell {
    position: relative;
    display: block;
    min-width: 0;
    aspect-ratio: 1;
    padding: 0;
    border: 1px solid var(--hairline-soft);
    border-radius: var(--radius-md);
    overflow: hidden;
    font: inherit;
  }
  .archived {
    background: var(--surface-2);
    border-color: var(--hairline);
    cursor: pointer;
    transition:
      border-color var(--motion-fast) var(--ease),
      transform var(--motion-fast) var(--ease);
  }
  .archived:active {
    transform: scale(0.96);
  }
  /* Inside the cell: `content-visibility` on the month clips anything drawn
   * outside it. Ink outline, then a white inner ring above the thumbnail,
   * so the ring reads on dark and light pictures alike. */
  .archived:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: -2px;
  }
  .archived:focus-visible::after {
    content: '';
    position: absolute;
    inset: 2px;
    z-index: 3;
    border: 2px solid var(--focus-halo);
    border-radius: calc(var(--radius-md) - 3px);
    pointer-events: none;
  }
  .empty {
    background: var(--surface-1);
  }
  /* Before the first post or after today: nothing could have been missed. */
  .outside {
    background: transparent;
    border-color: transparent;
  }
  img {
    position: absolute;
    inset: 0;
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .missing {
    position: absolute;
    inset: 0;
    background: repeating-linear-gradient(
      45deg,
      var(--surface-1),
      var(--surface-1) 6px,
      var(--surface-3) 6px,
      var(--surface-3) 12px
    );
  }
  /* A soft scrim along the top keeps the white date legible on any picture. */
  .archived::before {
    content: '';
    position: absolute;
    inset: 0 0 auto;
    z-index: 1;
    height: 55%;
    background: linear-gradient(rgb(0 0 0 / 58%), rgb(0 0 0 / 0%));
    pointer-events: none;
  }
  /* Thumbnail, then scrim, then labels, then the focus ring. */
  .date,
  .more,
  .daynum {
    z-index: 2;
  }
  .date,
  .more {
    position: absolute;
    top: 2px;
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    font-variant-numeric: tabular-nums;
    line-height: 1.4;
  }
  .date {
    left: 4px;
  }
  .more {
    right: 4px;
    color: #ffffff;
  }
  .archived .date {
    color: #ffffff;
  }
  .empty .date,
  .outside .date {
    color: var(--ink-subtle);
  }
  /* Today: the date sits in an ink dot, as calendars usually mark it. */
  .today .date {
    top: 2px;
    left: 2px;
    min-width: 1.4em;
    padding: 0 3px;
    color: var(--surface-1);
    text-align: center;
    background: var(--ink);
    border-radius: var(--radius-pill);
  }
  .daynum {
    position: absolute;
    right: 2px;
    bottom: 2px;
    padding: 0 4px;
    color: #ffffff;
    font-size: var(--fs-eyebrow);
    font-variant-numeric: tabular-nums;
    line-height: 1.4;
    background: rgb(0 0 0 / 62%);
    border-radius: var(--radius-sm);
  }
  @media (hover: hover) {
    .archived:hover {
      border-color: var(--border-strong);
    }
  }
</style>
