<script lang="ts">
  import type { Eligibility, Series } from '../../types/api';
  import Button from '../ui/Button.svelte';
  import ViolationCallout from '../creator/ViolationCallout.svelte';
  import Callout from '../shared/Callout.svelte';
  import SeriesCard from './SeriesCard.svelte';

  interface Props {
    series: Series[];
    onSelect: (series: Series) => void;
    /** Whether the viewer can start a series here; `null` while unknown. */
    eligibility?: Eligibility | null;
    /** Load state for {@link eligibility}. */
    eligibilityStatus?: 'loading' | 'ready' | 'failed';
    /** Whether the viewer owns at least one series shown here. */
    ownsSeries?: boolean;
    onCreate?: () => void;
    onManage?: () => void;
  }
  let {
    series,
    onSelect,
    eligibility = null,
    eligibilityStatus = 'loading',
    ownsSeries = false,
    onCreate,
    onManage,
  }: Props = $props();

  const canCreate = $derived(eligibility?.can_create ?? false);
  const blockers = $derived(eligibility && !eligibility.can_create ? eligibility.violations : []);
  /** Show the CTA when allowed, or when the check failed (create screen re-validates). */
  const showCreate = $derived(canCreate || eligibilityStatus === 'failed');
</script>

<div class="picker">
  <header class="head">
    <p class="eyebrow">Series</p>
    {#if ownsSeries && onManage}
      <button class="link" onclick={onManage}>Manage my series</button>
    {/if}
  </header>

  {#if series.length === 0}
    <Callout title="No series here yet">
      {#if canCreate}
        Start the first one — it shows up here as you post.
      {:else}
        Series created in this server show up here.
      {/if}
    </Callout>
  {:else}
    <ul class="list">
      {#each series as s (s.id)}
        <li><SeriesCard series={s} {onSelect} /></li>
      {/each}
    </ul>
  {/if}

  {#if showCreate && onCreate}
    <Button variant="primary" full disabled={eligibilityStatus === 'loading'} onclick={onCreate}>
      {eligibilityStatus === 'loading' ? 'Checking…' : 'Start a series'}
    </Button>
  {:else if blockers.length > 0}
    <ViolationCallout violations={blockers} />
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
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
  }
  .eyebrow {
    margin: 0;
    color: var(--ink-subtle);
    font-size: var(--fs-eyebrow);
    font-weight: var(--fw-emphasis);
    letter-spacing: 0.6px;
    text-transform: uppercase;
  }
  .link {
    padding: 0;
    color: var(--link);
    font: inherit;
    font-size: var(--fs-body-sm);
    background: none;
    border: 0;
    cursor: pointer;
  }
  .link:hover {
    text-decoration: underline;
  }
  .list {
    display: grid;
    gap: var(--space-sm);
    margin: 0;
    padding: 0;
    list-style: none;
  }
</style>
