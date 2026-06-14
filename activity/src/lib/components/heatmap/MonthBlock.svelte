<script lang="ts">
  import type { DaySummary } from '../../types/api';
  import { blockGrid, type Block, type GridContext } from '../../utils/heatmap';
  import ThumbTile from './ThumbTile.svelte';

  interface Props {
    block: Block;
    maxDay: number;
    startDay: number;
    load: (from: number, to: number) => Promise<DaySummary[]>;
    onOpenDay: (day: number) => void;
  }
  let { block, maxDay, startDay, load, onOpenDay }: Props = $props();

  let el: HTMLElement | undefined;
  let visible = $state(false);
  let zoomed = $state(false);
  let summaries = $state<DaySummary[] | null>(null);
  let failed = $state(false);

  // Mount this block's cells only near the viewport — bounds the live DOM
  // over multi-year archives. The block keeps a fixed height regardless, so
  // the scrollbar stays stable.
  $effect(() => {
    if (!el) return;
    const io = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) visible = true;
        }
      },
      { rootMargin: '600px 0px' },
    );
    io.observe(el);
    return () => io.disconnect();
  });

  // Fetch presence (and thumbnails) once visible, exactly once.
  $effect(() => {
    if (!visible || summaries !== null || failed) return;
    const to = Math.min(block.toDay, maxDay);
    if (block.fromDay > to) {
      summaries = [];
      return;
    }
    let cancelled = false;
    load(block.fromDay, to)
      .then((rows) => {
        if (!cancelled) summaries = rows;
      })
      .catch(() => {
        if (!cancelled) failed = true;
      });
    return () => {
      cancelled = true;
    };
  });

  const present = $derived(new Set((summaries ?? []).map((s) => s.day)));
  const ctx = $derived<GridContext>({ maxDay, startDay, present });
  const cells = $derived(blockGrid(block, ctx));
  const thumbByDay = $derived(new Map((summaries ?? []).map((s) => [s.day, s.thumb_url])));
  const hasContent = $derived(block.fromDay <= maxDay);
</script>

<section class="block" bind:this={el} class:zoomed>
  <header class="head">
    <span class="label">Days {block.fromDay}–{Math.min(block.toDay, maxDay)}</span>
    {#if hasContent}
      <button class="zoom" onclick={() => (zoomed = !zoomed)} aria-pressed={zoomed}>
        {zoomed ? 'Overview' : 'Photos'}
      </button>
    {/if}
  </header>

  <div class="area">
    {#if !visible}
      <!-- windowed out; the fixed height holds the scroll position -->
    {:else if failed}
      <p class="err">Couldn’t load these days.</p>
    {:else if zoomed}
      <div class="thumbs">
        {#each cells as cell (cell.day)}
          {#if cell.state === 'present'}
            <ThumbTile
              day={cell.day}
              thumbUrl={thumbByDay.get(cell.day) ?? null}
              onOpen={onOpenDay}
            />
          {/if}
        {/each}
      </div>
    {:else}
      <div class="grid" aria-label={`Days ${block.fromDay} to ${block.toDay}`}>
        {#each cells as cell (cell.day)}
          {#if cell.state === 'present'}
            <button
              class="cell present"
              aria-label={`Day ${cell.day}, archived`}
              onclick={() => onOpenDay(cell.day)}
            ></button>
          {:else}
            <span class="cell {cell.state}" aria-hidden="true"></span>
          {/if}
        {/each}
      </div>
    {/if}
  </div>
</section>

<style>
  .block {
    padding: var(--space-xs) 0;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: var(--space-xxs);
  }
  .label {
    color: var(--ink-subtle);
    font-size: var(--fs-caption);
  }
  .zoom {
    padding: 3px 10px;
    color: var(--ink-muted);
    font: inherit;
    font-size: var(--fs-caption);
    background: var(--surface-2);
    border: 0;
    border-radius: var(--radius-pill);
    cursor: pointer;
  }
  .zoom:active {
    background: var(--surface-3);
  }
  .area {
    min-height: var(--grid-h, 137px);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(7, var(--cell, 32px));
    gap: var(--cell-gap, 3px);
    width: max-content;
  }
  .cell {
    width: var(--cell, 32px);
    height: var(--cell, 32px);
    padding: 0;
    border: 0;
    border-radius: 3px;
  }
  .cell.present {
    background: var(--accent);
    cursor: pointer;
  }
  .cell.missing {
    background: var(--surface-2);
  }
  .cell.pre-start {
    background: var(--surface-1);
    border: 1px solid var(--hairline);
  }
  .cell.future {
    background: transparent;
  }
  .thumbs {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(56px, 1fr));
    gap: var(--space-xs);
    max-width: 480px;
  }
  .err {
    color: var(--ink-muted);
    font-size: var(--fs-body-sm);
  }
</style>
