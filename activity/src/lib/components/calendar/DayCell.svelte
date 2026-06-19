<script lang="ts">
  import type { MonthCell } from '../../utils/calendar';

  interface Props {
    cell: MonthCell;
    onOpenDay: (day: number) => void;
  }
  let { cell, onOpenDay }: Props = $props();
</script>

{#if cell.entry}
  {@const entry = cell.entry}
  <button class="cell present" onclick={() => onOpenDay(entry.day)} aria-label={`Day ${entry.day}`}>
    {#if entry.thumbUrl}
      <img src={entry.thumbUrl} alt="" loading="lazy" decoding="async" width="120" height="120" />
    {:else}
      <span class="missing" aria-hidden="true"></span>
    {/if}
    <span class="date">{cell.date}</span>
    <span class="daynum">{entry.day}</span>
  </button>
{:else}
  <span class="cell empty"><span class="date">{cell.date}</span></span>
{/if}

<style>
  .cell {
    position: relative;
    display: block;
    aspect-ratio: 1;
    padding: 0;
    border: 1px solid var(--hairline-soft);
    border-radius: var(--radius-md);
    overflow: hidden;
  }
  .present {
    background: var(--surface-2);
    border-color: var(--hairline);
    cursor: pointer;
    transition: border-color var(--motion-fast) var(--ease);
  }
  .present:hover,
  .present:focus-visible {
    border-color: var(--accent);
    outline: none;
  }
  .empty {
    background: var(--surface-1);
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
      var(--surface-2) 6px,
      var(--surface-2) 12px
    );
  }
  .date {
    position: absolute;
    top: 3px;
    left: 5px;
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    font-variant-numeric: tabular-nums;
  }
  .present .date {
    color: #ffffff;
    text-shadow: 0 1px 3px rgb(0 0 0 / 75%);
  }
  .empty .date {
    color: var(--ink-subtle);
  }
  .daynum {
    position: absolute;
    right: 4px;
    bottom: 3px;
    padding: 0 5px;
    color: #ffffff;
    font-size: 0.625rem; /* 10 */
    font-variant-numeric: tabular-nums;
    background: rgb(0 0 0 / 55%);
    border-radius: var(--radius-sm);
  }
</style>
