<script lang="ts">
  import type { DaySummary } from '../../types/api';
  import { monthBlocks, YEAR_BLOCKS } from '../../utils/heatmap';
  import MonthBlock from './MonthBlock.svelte';

  interface Props {
    maxDay: number;
    startDay: number;
    load: (from: number, to: number) => Promise<DaySummary[]>;
    onOpenDay: (day: number) => void;
  }
  let { maxDay, startDay, load, onOpenDay }: Props = $props();

  const blocks = $derived(monthBlocks(maxDay));
</script>

<div class="heatmap">
  {#each blocks as block (block.index)}
    {#if block.index % YEAR_BLOCKS === 0}
      <p class="year-sep"><span>Year {block.year}</span></p>
    {/if}
    <MonthBlock {block} {maxDay} {startDay} {load} {onOpenDay} />
  {/each}
</div>

<style>
  .heatmap {
    /* Heatmap layout constants; MonthBlock cells inherit these. */
    --cell: 32px;
    --cell-gap: 3px;
    --grid-h: 137px;
    display: grid;
    gap: var(--space-xxs);
  }
  .year-sep {
    display: flex;
    gap: var(--space-sm);
    align-items: center;
    margin: var(--space-md) 0 var(--space-xxs);
    color: var(--ink-subtle);
    font-size: var(--fs-caption);
  }
  .year-sep::before,
  .year-sep::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--hairline);
  }
</style>
