<script lang="ts">
  import type { Series } from '../../types/api';
  import Callout from '../shared/Callout.svelte';
  import SeriesCard from './SeriesCard.svelte';

  interface Props {
    series: Series[];
    onSelect: (series: Series) => void;
  }
  let { series, onSelect }: Props = $props();
</script>

<div class="picker">
  <p class="eyebrow">Series</p>
  {#if series.length === 0}
    <Callout title="No series here yet">
      Start one with <code>/series create</code> and your archive shows up here.
    </Callout>
  {:else}
    <ul class="list">
      {#each series as s (s.id)}
        <li><SeriesCard series={s} {onSelect} /></li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .picker {
    display: grid;
    gap: var(--space-md);
    width: 100%;
    max-width: 40rem;
    margin: 0 auto;
    padding: var(--space-lg);
  }
  .eyebrow {
    margin: 0;
    color: var(--ink-subtle);
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    letter-spacing: 0.6px;
    text-transform: uppercase;
  }
  .list {
    display: grid;
    gap: var(--space-sm);
    margin: 0;
    padding: 0;
    list-style: none;
  }
  code {
    padding: 1px 6px;
    font-size: 0.9em;
    background: var(--surface-2);
    border-radius: var(--radius-sm);
  }
</style>
